/*!
Spatio-temporal Reservoir Resample (aka ReSTIR) example.

The 2D world consists of:
    - ground surface receiving the light
    - occluder (a line segment at specified height)
    - sun (a unit sphere at specified location)
    - sky (the rest of the hemisphere)

This example implements ReSTIR for this world,
and shows the averaged (over time) brightness
for each point on the ground.
!*/

use std::{ops::Range, time::Duration};

struct Output {
    terminal: tui::Terminal<tui::backend::CrosstermBackend<std::io::Stdout>>,
}

impl Output {
    fn grab() -> Result<Self, std::io::Error> {
        use crossterm::ExecutableCommand as _;

        let mut stdout = std::io::stdout();
        stdout.execute(crossterm::terminal::EnterAlternateScreen)?;
        stdout.execute(crossterm::event::EnableMouseCapture)?;
        crossterm::terminal::enable_raw_mode()?;

        let backend = tui::backend::CrosstermBackend::new(stdout);
        let mut terminal = tui::Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok(Self { terminal })
    }

    fn release(&mut self) -> Result<(), std::io::Error> {
        use crossterm::ExecutableCommand as _;

        if std::thread::panicking() {
            // give the opportunity to see the result
            let _ = crossterm::event::read();
        }

        crossterm::terminal::disable_raw_mode()?;
        self.terminal
            .backend_mut()
            .execute(crossterm::event::DisableMouseCapture)?;
        self.terminal
            .backend_mut()
            .execute(crossterm::terminal::LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

struct WorldConfig {
    surface_length: u16,
    sun_position: [u16; 2],
    sun_color: [f32; 3],
    sky_color: [f32; 3],
    occluder_y: u16,
    occluder_x: Range<u16>,
}

#[derive(Clone, Default)]
struct SampleInfo {
    dir: glam::Vec2,
    distance: Option<f32>,
}

impl SampleInfo {
    /// Perform a "shift mapping" of one domain to another.
    fn shift_map(&self, src_origin: glam::Vec2, dst_origin: glam::Vec2) -> glam::Vec2 {
        match self.distance {
            Some(distance) => (src_origin + distance * self.dir - dst_origin).normalize(),
            None => self.dir,
        }
    }
}

#[derive(Default)]
struct Pixel {
    reservoir: rs_voir::Reservoir,
    selected_sample: SampleInfo,
    color: glam::Vec3,
    color_accumulated: glam::Vec3,
    variance_accumulated: f32,
}

struct WorldView<'a> {
    config: &'a WorldConfig,
    pixels: &'a [Pixel],
}
impl tui::widgets::Widget for WorldView<'_> {
    fn render(self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        use tui::style::Color;

        if area.height == 0 {
            return;
        }

        let bottom = area.y + area.height - 1;
        {
            let cell_index = (bottom - self.config.sun_position[1]) * buf.area.width
                + self.config.sun_position[0]
                + area.x;
            buf.content[cell_index as usize] = tui::buffer::Cell {
                symbol: "*".to_string(),
                fg: Color::Yellow,
                ..Default::default()
            };
        }

        for x in self.config.occluder_x.clone() {
            let cell_index = (bottom - self.config.occluder_y) * buf.area.width + x + area.x;
            buf.content[cell_index as usize] = tui::buffer::Cell {
                symbol: "=".to_string(),
                fg: Color::Blue,
                ..Default::default()
            };
        }

        for x in 0..self.config.surface_length {
            let cell_index = bottom * buf.area.width + x + area.x;
            buf.content[cell_index as usize] = tui::buffer::Cell {
                symbol: "-".to_string(),
                fg: Color::Green,
                ..Default::default()
            };
            if self.pixels[x as usize].color.length() > 1.0 {
                buf.content[(cell_index - buf.area.width) as usize] = tui::buffer::Cell {
                    symbol: ",".to_string(),
                    fg: Color::White,
                    ..Default::default()
                };
            }
        }
    }
}

#[allow(dead_code)]
enum Convergence {
    Precise { unbias: bool },
    LeanAndMean { initial_visibility: bool },
}

struct RestirConfig {
    convergence: Convergence,
    initial_samples: u32,
    max_initial_history: u32,
    max_temporal_history: u32,
    max_spatial_history: u32,
}

struct Config {
    world: WorldConfig,
    restir: RestirConfig,
    accumulation: f32,
}

#[derive(Default)]
struct LightInfo {
    color: glam::Vec3,
    distance: Option<f32>,
}

impl LightInfo {
    /// Returns the value we assign to this light, based on the color (or intensity).
    /// This is also known as "unnormalized target PDF" in literature.
    fn target_value(&self) -> f32 {
        self.color.length()
    }
}

impl WorldConfig {
    fn get_incoming_light(&self, origin: glam::Vec2, dir: glam::Vec2) -> LightInfo {
        debug_assert!(dir.is_normalized());
        let sun_pos = glam::vec2(
            self.sun_position[0] as f32 + 0.5,
            self.sun_position[1] as f32 + 0.5,
        );
        let diff = sun_pos - origin;
        let sun_distance = diff.dot(dir);
        let leftover = diff - sun_distance * dir;
        let sun_radius = 0.5;
        if leftover.length_squared() < sun_radius * sun_radius {
            LightInfo {
                color: self.sun_color.into(),
                distance: Some(sun_distance),
            }
        } else {
            LightInfo {
                color: self.sky_color.into(),
                distance: None,
            }
        }
    }

    fn check_visibility(&self, origin: glam::Vec2, dir: glam::Vec2) -> bool {
        if dir.y <= 0.0 {
            return false;
        }
        if origin.y > self.occluder_y as f32 {
            return true;
        }
        let t = (self.occluder_y as f32 + 0.5 - origin.y) / dir.y;
        let x = origin.x + dir.x * t;
        x < self.occluder_x.start as f32 || x > self.occluder_x.end as f32
    }
}

struct Render {
    config: Config,
    pixels: Box<[Pixel]>,
    random: rand::rngs::ThreadRng,
    frame_index: usize,
    smooth_avg_deviation: f32,
}
impl Render {
    fn update(&mut self) {
        use rand::Rng;
        use std::f32::consts::PI;

        self.frame_index += 1;

        // Back up the current information before re-using
        let backup = self
            .pixels
            .iter()
            .map(|pixel| (pixel.reservoir.clone(), pixel.selected_sample.clone()))
            .collect::<Vec<_>>();

        for (cell_index, pixel) in self.pixels.iter_mut().enumerate() {
            let surface_pos = glam::vec2(cell_index as f32 + 0.5, 0.0);
            let mut builder = rs_voir::ReservoirBuilder::default();
            let mut selected_dir = glam::Vec2::ZERO;
            let mut selected_linfo = LightInfo::default();

            // First, do RIS on the initial samples
            for _ in 0..self.config.restir.initial_samples {
                // generate a random direction in the hemisphere
                let alpha = self.random.gen_range(0.0..=PI);
                let dir = glam::vec2(alpha.cos(), alpha.sin());
                let is_visible = match self.config.restir.convergence {
                    Convergence::Precise { .. } => {
                        self.config.world.check_visibility(surface_pos, dir)
                    }
                    Convergence::LeanAndMean { .. } => true,
                };
                if is_visible {
                    let linfo = self.config.world.get_incoming_light(surface_pos, dir);
                    if builder.stream(1.0 / PI, linfo.target_value(), &mut self.random) {
                        selected_dir = dir;
                        selected_linfo = linfo;
                    }
                } else {
                    builder.add_empty_sample();
                }
            }

            if let Convergence::LeanAndMean {
                initial_visibility: true,
                ..
            } = self.config.restir.convergence
            {
                if !self
                    .config
                    .world
                    .check_visibility(surface_pos, selected_dir)
                {
                    selected_linfo = LightInfo::default();
                }
            }
            builder.clamp_history(self.config.restir.max_initial_history);

            // Second, reuse the previous frame reservoir.
            if self.config.restir.max_temporal_history != 0 {
                let (ref prev_reservoir, ref prev_sample) = backup[cell_index];
                let prev = prev_reservoir.with_max_history(self.config.restir.max_temporal_history);
                if prev.has_weight() {
                    // reconstruct the target PDF
                    let linfo = self
                        .config
                        .world
                        .get_incoming_light(surface_pos, pixel.selected_sample.dir);
                    let other = prev.to_builder(linfo.target_value());
                    if builder.merge(&other, &mut self.random) {
                        selected_dir = prev_sample.dir;
                        selected_linfo = linfo;
                    }
                } else {
                    builder.merge_history(&prev);
                }
            }

            // Third, reuse the previous frame neighboring reservoirs
            let mut unbiased_history = builder.history();
            if self.config.restir.max_spatial_history != 0 {
                let mut selected_cell = -1;
                for offset in [-1, 1] {
                    let index = cell_index as isize + offset;
                    if index < 0 || index >= self.config.world.surface_length as isize {
                        continue;
                    }
                    let (ref prev_reservoir, ref prev_sample) = backup[index as usize];
                    let prev =
                        prev_reservoir.with_max_history(self.config.restir.max_spatial_history);
                    let other_pos = surface_pos + glam::vec2(offset as f32, 0.0);

                    if prev.has_weight() {
                        let surface_dir = prev_sample.shift_map(other_pos, surface_pos);
                        let is_visible = match self.config.restir.convergence {
                            Convergence::Precise { .. } => {
                                self.config.world.check_visibility(surface_pos, surface_dir)
                            }
                            Convergence::LeanAndMean { .. } => true,
                        };
                        if is_visible {
                            // reconstruct the target PDF
                            let linfo = self
                                .config
                                .world
                                .get_incoming_light(surface_pos, surface_dir);
                            let other = prev.to_builder(linfo.target_value());
                            if builder.merge(&other, &mut self.random) {
                                selected_dir = surface_dir;
                                selected_linfo = linfo;
                                selected_cell = index;
                            }
                        } else {
                            builder.merge_history(&prev);
                        }
                    } else {
                        builder.merge_history(&prev);
                    }
                }

                // Post-factum reject reservoirs that couldn't have produced this sample.
                if let Convergence::Precise { unbias: true } = self.config.restir.convergence {
                    let selected_sample = SampleInfo {
                        dir: selected_dir,
                        distance: selected_linfo.distance,
                    };
                    for offset in [-1, 1] {
                        let index = cell_index as isize + offset;
                        if index < 0 || index >= self.config.world.surface_length as isize {
                            continue;
                        }
                        let (ref prev_reservoir, _) = backup[index as usize];
                        let covers_domain = if index == selected_cell {
                            true
                        } else {
                            let other_pos = surface_pos + glam::vec2(offset as f32, 0.0);
                            let other_dir = selected_sample.shift_map(surface_pos, other_pos);
                            self.config.world.check_visibility(other_pos, other_dir)
                        };
                        if covers_domain {
                            unbiased_history += prev_reservoir
                                .with_max_history(self.config.restir.max_spatial_history)
                                .history();
                        }
                    }
                } else {
                    unbiased_history = builder.history();
                }
            }

            if let Convergence::LeanAndMean { .. } = self.config.restir.convergence {
                if selected_linfo.target_value() > 0.0
                    && !self
                        .config
                        .world
                        .check_visibility(surface_pos, selected_dir)
                {
                    selected_linfo = LightInfo::default();
                }
            }

            // Finally write out the results
            pixel.reservoir = builder.finish_with_history(unbiased_history);
            pixel.selected_sample = SampleInfo {
                dir: selected_dir,
                distance: selected_linfo.distance,
            };

            pixel.color = selected_linfo.color * pixel.reservoir.contribution_weight();
            let variance = (pixel.color - pixel.color_accumulated).length_squared();
            pixel.variance_accumulated = pixel.variance_accumulated
                * (1.0 - self.config.accumulation)
                + self.config.accumulation * variance;
            pixel.color_accumulated = pixel.color_accumulated * (1.0 - self.config.accumulation)
                + self.config.accumulation * pixel.color;
        }

        let sum_variance = self
            .pixels
            .iter()
            .map(|pixel| pixel.variance_accumulated)
            .sum::<f32>();
        let std_deviation = (sum_variance / self.pixels.len() as f32).sqrt();
        self.smooth_avg_deviation = self.smooth_avg_deviation * (1.0 - self.config.accumulation)
            + self.config.accumulation * std_deviation;
    }

    fn draw<B: tui::backend::Backend>(&self, frame: &mut tui::Frame<B>) {
        use tui::{
            layout as l,
            style::{Color, Style},
            text::{Span, Spans},
            widgets as w,
        };

        fn make_key_value(key: &str, value: String) -> Spans {
            Spans(vec![
                Span::styled(key, Style::default().fg(Color::DarkGray)),
                Span::raw(value),
            ])
        }
        fn make_key_bool(key: &str, value: bool) -> Spans {
            let (color, value_str) = if value {
                (Color::Green, "on")
            } else {
                (Color::Red, "off")
            };
            Spans(vec![
                Span::styled(key, Style::default().fg(Color::DarkGray)),
                Span::styled(value_str, Style::default().fg(color)),
            ])
        }

        let top_hor_rects = l::Layout::default()
            .direction(l::Direction::Horizontal)
            .constraints(
                [
                    l::Constraint::Length((self.config.world.surface_length + 4) as _),
                    l::Constraint::Percentage(15),
                ]
                .as_ref(),
            )
            .margin(1)
            .split(frame.size());

        let top_vl_rects = l::Layout::default()
            .direction(l::Direction::Vertical)
            .constraints(
                [
                    l::Constraint::Length((self.config.world.sun_position[1] + 3) as _),
                    l::Constraint::Min(10),
                ]
                .as_ref(),
            )
            .margin(1)
            .split(top_hor_rects[0]);
        let top_vr_rects = l::Layout::default()
            .direction(l::Direction::Vertical)
            .constraints([l::Constraint::Length(5), l::Constraint::Length(10)].as_ref())
            .margin(1)
            .split(top_hor_rects[1]);

        let world_block = w::Block::default().borders(w::Borders::ALL).title("World");
        let inner = world_block.inner(top_vl_rects[0]);
        frame.render_widget(world_block, top_vl_rects[0]);
        frame.render_widget(
            WorldView {
                config: &self.config.world,
                pixels: &self.pixels,
            },
            inner,
        );

        let brightness_scale = 100u64;
        let brightness = self
            .pixels
            .iter()
            .map(|pixel| (pixel.color_accumulated.length() * brightness_scale as f32) as u64)
            .collect::<Vec<_>>();
        let brightness_block = w::Sparkline::default()
            .block(
                w::Block::default()
                    .title(format!("Brightness / {}", brightness_scale))
                    .borders(w::Borders::ALL),
            )
            .data(&brightness)
            .max(brightness_scale * 2);
        frame.render_widget(brightness_block, top_vl_rects[1]);

        let max_deviation = 5.0;
        let deviation_color = if self.smooth_avg_deviation < 1.0 {
            Color::Green
        } else {
            Color::Red
        };
        let deviation_block = w::Gauge::default()
            .block(
                w::Block::default()
                    .title("Std Deviation")
                    .borders(w::Borders::ALL),
            )
            .gauge_style(Style::default().fg(deviation_color).bg(Color::Black))
            .label(format!("{:.2}", self.smooth_avg_deviation))
            .ratio((self.smooth_avg_deviation / max_deviation).min(1.0) as f64);
        frame.render_widget(deviation_block, top_vr_rects[0]);

        let mut text = vec![
            make_key_value("Frame: ", format!("{}", self.frame_index)),
            make_key_value(
                "Initial samples: ",
                format!("{}", self.config.restir.initial_samples),
            ),
            make_key_value(
                "Convergence: ",
                match self.config.restir.convergence {
                    Convergence::Precise { .. } => "Precise".to_string(),
                    Convergence::LeanAndMean { .. } => "Lean&Mean".to_string(),
                },
            ),
        ];
        match self.config.restir.convergence {
            Convergence::Precise { unbias } => {
                text.push(make_key_bool("Unbias: ", unbias));
            }
            Convergence::LeanAndMean { initial_visibility } => {
                text.push(make_key_bool("Initial visibility: ", initial_visibility));
            }
        }
        let text_block = w::Paragraph::new(text)
            .block(w::Block::default().title("Info").borders(w::Borders::ALL))
            .wrap(w::Wrap { trim: true });
        frame.render_widget(text_block, top_vr_rects[1]);
    }
}

fn main() {
    use crossterm::event as ev;

    let surface_length = 40;
    let mut render = Render {
        config: Config {
            world: WorldConfig {
                surface_length,
                sun_position: [5, 10],
                sun_color: [10.0, 10.0, 1.0],
                sky_color: [0.0, 0.0, 0.1],
                occluder_y: 5,
                occluder_x: 7..15,
            },
            restir: RestirConfig {
                convergence: Convergence::Precise { unbias: true },
                //convergence: Convergence::LeanAndMean { initial_visibility: true },
                initial_samples: 4,
                max_initial_history: 1,
                max_temporal_history: 20,
                max_spatial_history: 10,
            },
            accumulation: 0.01,
        },
        pixels: (0..surface_length).map(|_| Pixel::default()).collect(),
        random: rand::thread_rng(),
        frame_index: 0,
        smooth_avg_deviation: 0.0,
    };

    let mut output = Output::grab().unwrap();
    loop {
        render.update();
        output.terminal.draw(|f| render.draw(f)).unwrap();

        while ev::poll(Duration::ZERO).unwrap() {
            match ev::read().unwrap() {
                ev::Event::Resize(..) => {}
                ev::Event::Key(event) => match event.code {
                    ev::KeyCode::Esc => {
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}
