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

struct Config {
    surface_length: u16,
    sun_position: [u16; 2],
    sun_intensity: f32,
    ambient_intensity: f32,
    occluder_y: u16,
    occluder_x: Range<u16>,
}

struct WorldView<'a> {
    config: &'a Config,
}
impl tui::widgets::Widget for WorldView<'_> {
    fn render(self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        use tui::style::Color;

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

struct Restir {
    config: Config,
}
impl Restir {
    fn update(&mut self) {}

    fn draw<B: tui::backend::Backend>(&self, frame: &mut tui::Frame<B>) {
        use tui::{
            layout as l,
            style::{Color, Style},
            text::{Span, Spans},
            widgets as w,
        };

        let top_hor_rects = l::Layout::default()
            .direction(l::Direction::Horizontal)
            .constraints(
                [
                    l::Constraint::Min(self.config.surface_length as _),
                    l::Constraint::Percentage(15),
                ]
                .as_ref(),
            )
            .margin(1)
            .split(frame.size());

        let top_ver_rects = l::Layout::default()
            .direction(l::Direction::Vertical)
            .constraints(
                [
                    l::Constraint::Min(self.config.sun_position[1] as _),
                    l::Constraint::Percentage(15),
                ]
                .as_ref(),
            )
            .margin(1)
            .split(top_hor_rects[0]);

        frame.render_widget(
            WorldView {
                config: &self.config,
            },
            top_ver_rects[0],
        );
    }
}

fn main() {
    use crossterm::event as ev;

    let mut restir = Restir {
        config: Config {
            surface_length: 20,
            sun_position: [5, 10],
            sun_intensity: 100.0,
            ambient_intensity: 1.0,
            occluder_y: 5,
            occluder_x: 7..15,
        },
    };
    let mut output = Output::grab().unwrap();
    loop {
        restir.update();
        output.terminal.draw(|f| restir.draw(f)).unwrap();

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
