use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub(crate) type KbmdTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Owns every process-global terminal mode enabled by the TUI.
///
/// Cleanup is best-effort in `Drop`, including while unwinding from a panic. The setup guard also
/// rolls back partial initialization if a later setup step fails.
pub(crate) struct TerminalSession {
    terminal: KbmdTerminal,
}

impl TerminalSession {
    pub fn enter() -> Result<Self> {
        enable_raw_mode().context("could not enable terminal raw mode")?;
        let mut setup = SetupGuard {
            raw: true,
            alternate: false,
            mouse: false,
            hidden_cursor: false,
        };

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("could not enter alternate screen")?;
        setup.alternate = true;
        execute!(stdout, EnableMouseCapture).context("could not enable mouse capture")?;
        setup.mouse = true;
        execute!(stdout, Hide).context("could not hide cursor")?;
        setup.hidden_cursor = true;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("could not initialize terminal")?;
        setup.disarm();
        Ok(Self { terminal })
    }

    pub fn terminal_mut(&mut self) -> &mut KbmdTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Keep cleanup steps independent so one failed escape-sequence write does not prevent the
        // remaining process-global modes from being restored.
        let _ = execute!(self.terminal.backend_mut(), Show);
        let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

struct SetupGuard {
    raw: bool,
    alternate: bool,
    mouse: bool,
    hidden_cursor: bool,
}

impl SetupGuard {
    fn disarm(&mut self) {
        self.raw = false;
        self.alternate = false;
        self.mouse = false;
        self.hidden_cursor = false;
    }
}

impl Drop for SetupGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.hidden_cursor {
            let _ = execute!(stdout, Show);
        }
        if self.mouse {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.alternate {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        if self.raw {
            let _ = disable_raw_mode();
        }
    }
}
