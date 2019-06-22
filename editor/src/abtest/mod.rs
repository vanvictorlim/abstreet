mod score;
pub mod setup;

use crate::common::{CommonState, SpeedControls};
use crate::render::MIN_ZOOM_FOR_DETAIL;
use crate::state::{State, Transition};
use crate::ui::{PerMapUI, ShowEverything, UI};
use ezgui::{hotkey, Color, EventCtx, EventLoopMode, GeomBatch, GfxCtx, Key, ModalMenu, Text};
use geom::{Circle, Distance, Duration, Line, PolyLine};
use map_model::{Map, LANE_THICKNESS};
use serde_derive::{Deserialize, Serialize};
use sim::{Sim, TripID};

pub struct ABTestMode {
    menu: ModalMenu,
    speed: SpeedControls,
    // TODO Urgh, hack. Need to be able to take() it to switch states sometimes.
    secondary: Option<PerMapUI>,
    diff_trip: Option<DiffOneTrip>,
    diff_all: Option<DiffAllTrips>,
    // TODO Not present in Setup state.
    common: CommonState,
    test_name: String,
}

impl ABTestMode {
    pub fn new(
        ctx: &mut EventCtx,
        ui: &mut UI,
        test_name: &str,
        secondary: PerMapUI,
    ) -> ABTestMode {
        ui.primary.current_selection = None;

        ABTestMode {
            menu: ModalMenu::new(
                "A/B Test Mode",
                vec![
                    vec![
                        (hotkey(Key::Escape), "quit"),
                        (hotkey(Key::LeftBracket), "slow down"),
                        (hotkey(Key::RightBracket), "speed up"),
                        (hotkey(Key::Space), "pause/resume"),
                        (hotkey(Key::M), "step forwards 0.1s"),
                        (hotkey(Key::S), "swap"),
                        (hotkey(Key::D), "diff all trips"),
                        (hotkey(Key::B), "stop diffing trips"),
                        (hotkey(Key::Q), "scoreboard"),
                        (hotkey(Key::O), "save state"),
                    ],
                    CommonState::modal_menu_entries(),
                ]
                .concat(),
                ctx,
            ),
            speed: SpeedControls::new(ctx, None),
            secondary: Some(secondary),
            diff_trip: None,
            diff_all: None,
            common: CommonState::new(),
            test_name: test_name.to_string(),
        }
    }
}

impl State for ABTestMode {
    fn event(&mut self, ctx: &mut EventCtx, ui: &mut UI) -> (Transition, EventLoopMode) {
        let mut txt = Text::prompt("A/B Test Mode");
        txt.add_line(ui.primary.map.get_edits().edits_name.clone());
        if let Some(ref diff) = self.diff_trip {
            txt.add_line(format!("Showing diff for {}", diff.trip));
        } else if let Some(ref diff) = self.diff_all {
            txt.add_line(format!(
                "Showing diffs for all. {} trips same, {} differ",
                diff.same_trips,
                diff.lines.len()
            ));
        }
        txt.add_line(ui.primary.sim.summary());
        self.menu.handle_event(ctx, Some(txt));

        ctx.canvas.handle_event(ctx.input);
        if ctx.redo_mouseover() {
            ui.primary.current_selection = ui.recalculate_current_selection(
                ctx,
                &ui.primary.sim,
                &ShowEverything::new(),
                false,
            );
        }
        if let Some(evmode) = self.common.event(ctx, ui, &mut self.menu) {
            return (Transition::Keep, evmode);
        }

        if self.menu.action("quit") {
            // TODO Should we clear edits too?
            ui.primary.reset_sim();
            // Note destroying mode.secondary has some noticeable delay.
            return (Transition::Pop, EventLoopMode::InputOnly);
        }

        if self.menu.action("swap") {
            let secondary = self.secondary.take().unwrap();
            let primary = std::mem::replace(&mut ui.primary, secondary);
            self.secondary = Some(primary);
            self.recalculate_stuff(ui, ctx);
        }

        if self.menu.action("scoreboard") {
            self.speed.pause();
            return (
                Transition::Push(Box::new(score::Scoreboard::new(
                    ctx,
                    &ui.primary,
                    self.secondary.as_ref().unwrap(),
                ))),
                EventLoopMode::InputOnly,
            );
        }

        if self.menu.action("save state") {
            self.savestate(&mut ui.primary);
        }

        if self.diff_trip.is_some() {
            if self.menu.action("stop diffing trips") {
                self.diff_trip = None;
            }
        } else if self.diff_all.is_some() {
            if self.menu.action("stop diffing trips") {
                self.diff_all = None;
            }
        } else {
            if ui.primary.current_selection.is_none() && self.menu.action("diff all trips") {
                self.diff_all = Some(DiffAllTrips::new(
                    &mut ui.primary,
                    self.secondary.as_mut().unwrap(),
                ));
            } else if let Some(agent) = ui.primary.current_selection.and_then(|id| id.agent_id()) {
                if let Some(trip) = ui.primary.sim.agent_to_trip(agent) {
                    if ctx
                        .input
                        .contextual_action(Key::B, &format!("Show {}'s parallel world", agent))
                    {
                        self.diff_trip = Some(DiffOneTrip::new(
                            trip,
                            &ui.primary,
                            self.secondary.as_ref().unwrap(),
                        ));
                    }
                }
            }
        }

        if let Some(dt) = self.speed.event(ctx, &mut self.menu, ui.primary.sim.time()) {
            self.step(dt, ui, ctx);
        }

        if self.speed.is_paused() {
            if self.menu.action("step forwards 0.1s") {
                self.step(Duration::seconds(0.1), ui, ctx);
            }
            (Transition::Keep, EventLoopMode::InputOnly)
        } else {
            (Transition::Keep, EventLoopMode::Animation)
        }
    }

    fn draw(&self, g: &mut GfxCtx, ui: &UI) {
        self.common.draw(g, ui);

        if let Some(ref diff) = self.diff_trip {
            diff.draw(g, ui);
        }
        if let Some(ref diff) = self.diff_all {
            diff.draw(g, ui);
        }
        self.menu.draw(g);
        self.speed.draw(g);
    }
}

impl ABTestMode {
    fn step(&mut self, dt: Duration, ui: &mut UI, ctx: &EventCtx) {
        ui.primary.sim.step(&ui.primary.map, dt);
        {
            let s = self.secondary.as_mut().unwrap();
            s.sim.step(&s.map, dt);
        }
        self.recalculate_stuff(ui, ctx);
    }

    fn recalculate_stuff(&mut self, ui: &mut UI, ctx: &EventCtx) {
        if let Some(diff) = self.diff_trip.take() {
            self.diff_trip = Some(DiffOneTrip::new(
                diff.trip,
                &ui.primary,
                self.secondary.as_ref().unwrap(),
            ));
        }
        if self.diff_all.is_some() {
            self.diff_all = Some(DiffAllTrips::new(
                &mut ui.primary,
                self.secondary.as_mut().unwrap(),
            ));
        }

        ui.primary.current_selection =
            ui.recalculate_current_selection(ctx, &ui.primary.sim, &ShowEverything::new(), false);
    }

    fn savestate(&mut self, primary: &mut PerMapUI) {
        // Temporarily move everything into this structure.
        let blank_map = Map::blank();
        let mut secondary = self.secondary.take().unwrap();
        let ss = ABTestSavestate {
            primary_map: std::mem::replace(&mut primary.map, Map::blank()),
            primary_sim: std::mem::replace(
                &mut primary.sim,
                Sim::new(&blank_map, "run".to_string(), None),
            ),
            secondary_map: std::mem::replace(&mut secondary.map, Map::blank()),
            secondary_sim: std::mem::replace(
                &mut secondary.sim,
                Sim::new(&blank_map, "run".to_string(), None),
            ),
        };

        let path = format!(
            "../data/ab_test_saves/{}/{}/{}.bin",
            ss.primary_map.get_name(),
            self.test_name,
            ss.primary_sim.time()
        );
        abstutil::write_binary(&path, &ss).unwrap();
        println!("Saved {}", path);

        // Restore everything.
        primary.sim = ss.primary_sim;
        primary.map = ss.primary_map;
        self.secondary = Some(PerMapUI {
            map: ss.secondary_map,
            draw_map: secondary.draw_map,
            sim: ss.secondary_sim,
            current_selection: secondary.current_selection,
            current_flags: secondary.current_flags,
        });
    }
}

pub struct DiffOneTrip {
    trip: TripID,
    // These are all optional because mode-changes might cause temporary interruptions.
    // Just point from primary world agent to secondary world agent.
    line: Option<Line>,
    primary_route: Option<PolyLine>,
    secondary_route: Option<PolyLine>,
}

impl DiffOneTrip {
    fn new(trip: TripID, primary: &PerMapUI, secondary: &PerMapUI) -> DiffOneTrip {
        let pt1 = primary.sim.get_canonical_pt_per_trip(trip, &primary.map);
        let pt2 = secondary
            .sim
            .get_canonical_pt_per_trip(trip, &secondary.map);
        let line = if pt1.is_some() && pt2.is_some() {
            Line::maybe_new(pt1.unwrap(), pt2.unwrap())
        } else {
            None
        };
        let primary_agent = primary.sim.trip_to_agent(trip);
        let secondary_agent = secondary.sim.trip_to_agent(trip);
        if primary_agent.is_none() || secondary_agent.is_none() {
            println!("{} isn't present in both sims", trip);
        }
        DiffOneTrip {
            trip,
            line,
            primary_route: primary_agent
                .and_then(|a| primary.sim.trace_route(a, &primary.map, None)),
            secondary_route: secondary_agent
                .and_then(|a| secondary.sim.trace_route(a, &secondary.map, None)),
        }
    }

    fn draw(&self, g: &mut GfxCtx, ui: &UI) {
        if let Some(l) = &self.line {
            g.draw_line(
                ui.cs.get_def("diff agents line", Color::YELLOW.alpha(0.5)),
                LANE_THICKNESS,
                l,
            );
        }
        if let Some(t) = &self.primary_route {
            g.draw_polygon(
                ui.cs.get_def("primary agent route", Color::RED.alpha(0.5)),
                &t.make_polygons(LANE_THICKNESS),
            );
        }
        if let Some(t) = &self.secondary_route {
            g.draw_polygon(
                ui.cs
                    .get_def("secondary agent route", Color::BLUE.alpha(0.5)),
                &t.make_polygons(LANE_THICKNESS),
            );
        }
    }
}

pub struct DiffAllTrips {
    same_trips: usize,
    // TODO Or do we want to augment DrawCars and DrawPeds, so we get automatic quadtree support?
    lines: Vec<Line>,
}

impl DiffAllTrips {
    fn new(primary: &mut PerMapUI, secondary: &mut PerMapUI) -> DiffAllTrips {
        let trip_positions1 = primary.sim.get_trip_positions(&primary.map);
        let trip_positions2 = secondary.sim.get_trip_positions(&secondary.map);
        let mut same_trips = 0;
        let mut lines: Vec<Line> = Vec::new();
        for (trip, pt1) in &trip_positions1.canonical_pt_per_trip {
            if let Some(pt2) = trip_positions2.canonical_pt_per_trip.get(trip) {
                if let Some(l) = Line::maybe_new(*pt1, *pt2) {
                    lines.push(l);
                } else {
                    same_trips += 1;
                }
            }
        }
        DiffAllTrips { same_trips, lines }
    }

    fn draw(&self, g: &mut GfxCtx, ui: &UI) {
        let mut batch = GeomBatch::new();
        let color = ui.cs.get("diff agents line");
        if g.canvas.cam_zoom < MIN_ZOOM_FOR_DETAIL {
            // TODO Refactor with UI
            let radius = Distance::meters(10.0) / g.canvas.cam_zoom;
            for line in &self.lines {
                batch.push(color, Circle::new(line.pt1(), radius).to_polygon());
            }
        } else {
            for line in &self.lines {
                batch.push(color, line.make_polygons(LANE_THICKNESS));
            }
        }
        batch.draw(g);
    }
}

#[derive(Serialize, Deserialize)]
pub struct ABTestSavestate {
    primary_map: Map,
    primary_sim: Sim,
    secondary_map: Map,
    secondary_sim: Sim,
}
