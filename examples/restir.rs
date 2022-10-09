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
    sun_intensity: f32,
    sky_intensity: f32,
    occluder_y: u16,
    occluder_x: Range<u16>,
}

struct WorldView<'a> {
    config: &'a WorldConfig,
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
                symbol: "_".to_string(),
                fg: Color::Green,
                ..Default::default()
            };
        }
    }
}

#[derive(Default)]
struct Pixel {
    reservoir: rs_voir::Reservoir,
    selected_dir: glam::Vec2,
    brightness: f32,
    brightness_accumulated: f32,
}

struct RestirConfig {
    initial_samples: u32,
    initial_visibility: bool,
    reuse_temporal: bool,
    reuse_spatial: bool,
    max_history: u32,
}

struct Config {
    world: WorldConfig,
    restir: RestirConfig,
    accumulation: f32,
}

impl WorldConfig {
    fn get_incoming_light(&self, origin: glam::Vec2, dir: glam::Vec2) -> f32 {
        debug_assert!(dir.is_normalized());
        let sun_pos = glam::vec2(
            self.sun_position[0] as f32 + 0.5,
            self.sun_position[1] as f32 + 0.5,
        );
        let diff = sun_pos - origin;
        let leftover = diff - diff.dot(dir) * dir;
        let sun_radius = 0.5;
        if leftover.length_squared() < sun_radius * sun_radius {
            self.sun_intensity
        } else {
            self.sky_intensity
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
}
impl Render {
    fn update(&mut self) {
        use rand::Rng;
        use std::f32::consts::PI;

        self.frame_index += 1;

        for (cell_index, pixel) in self.pixels.iter_mut().enumerate() {
            let surface_pos = glam::vec2(cell_index as f32 + 0.5, 0.0);
            let mut builder = rs_voir::ReservoirBuilder::default();
            let mut selected_dir = glam::Vec2::ZERO;
            let mut selected_intencity = 0.0;
            // First, do RIS on the initial samples
            for _ in 0..self.config.restir.initial_samples {
                // generate a random direction in the hemisphere
                let alpha = self.random.gen_range(0.0..=PI);
                let dir = glam::vec2(alpha.cos(), alpha.sin());
                if self.config.restir.initial_visibility
                    && !self.config.world.check_visibility(surface_pos, dir)
                {
                    builder.add_empty_sample();
                } else {
                    let intensity = self.config.world.get_incoming_light(surface_pos, dir);
                    if builder.stream(1.0 / PI, intensity, &mut self.random) {
                        selected_dir = dir;
                        selected_intencity = intensity;
                    }
                }
            }
            // From now on, we consider visibility to be a part of the target PDF.
            // Therefore, if the given reservoir was built without visibility checks,
            // it's time to collapse it to one sample, which we also check now.
            if !self.config.restir.initial_visibility {
                if !self
                    .config
                    .world
                    .check_visibility(surface_pos, selected_dir)
                {
                    selected_intencity = 0.0;
                }
                builder.collapse();
            }

            // Second, reuse the previous frame reservoir
            if self.config.restir.reuse_temporal {}

            // Third, reuse the previous frame neighboring reservoirs
            if self.config.restir.reuse_spatial {}

            // Finally write out the results
            pixel.reservoir = builder.finish(self.config.restir.max_history);
            pixel.selected_dir = selected_dir;
            pixel.brightness = selected_intencity * pixel.reservoir.contribution_weight();
            pixel.brightness_accumulated = pixel.brightness_accumulated
                * (1.0 - self.config.accumulation)
                + self.config.accumulation * pixel.brightness;
        }
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

        let info_block = w::Paragraph::new(vec![make_key_value(
            "Frame: ",
            format!("{}", self.frame_index),
        )])
        .block(w::Block::default().title("Info").borders(w::Borders::ALL))
        .wrap(w::Wrap { trim: true });
        frame.render_widget(info_block, top_hor_rects[1]);

        let top_ver_rects = l::Layout::default()
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

        let world_block = w::Block::default().borders(w::Borders::ALL).title("World");
        let inner = world_block.inner(top_ver_rects[0]);
        frame.render_widget(world_block, top_ver_rects[0]);
        frame.render_widget(
            WorldView {
                config: &self.config.world,
            },
            inner,
        );

        let brightness = self
            .pixels
            .iter()
            .map(|pixel| ("", pixel.brightness_accumulated as u64))
            .collect::<Vec<_>>();
        let chart_block = w::BarChart::default()
            .block(
                w::Block::default()
                    .title("Brightness")
                    .borders(w::Borders::ALL),
            )
            .data(&brightness)
            .bar_width(1)
            .bar_gap(0)
            //.bar_style(Style::default().fg(Color::Yellow))
            .value_style(Style::default().fg(Color::Black).bg(Color::Yellow));
        frame.render_widget(chart_block, top_ver_rects[1]);
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
                sun_intensity: 100.0,
                sky_intensity: 1.0,
                occluder_y: 5,
                occluder_x: 7..15,
            },
            restir: RestirConfig {
                initial_samples: 4,
                initial_visibility: true,
                reuse_temporal: false,
                reuse_spatial: false,
                max_history: 20,
            },
            accumulation: 0.01,
        },
        pixels: (0..surface_length).map(|_| Pixel::default()).collect(),
        random: rand::thread_rng(),
        frame_index: 0,
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
