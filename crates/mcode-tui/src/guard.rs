//! RAII terminal restore for raw mode and the alternate screen.
//!
//! [`TerminalGuard`] records entered modes and restores them on Drop, on
//! explicit [`restore_on_abnormal_exit`], and from a panic hook installed by
//! [`TerminalGuard::enter`]. Tests use [`TerminalGuard::new_mocked`] so the
//! real console is never touched. Native crossterm calls run only on
//! [`TerminalGuard::enter`].

// Rust guideline compliant 2026-08-27.

use std::io::{self, stdout};
use std::panic;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, Weak};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

static ACTIVE_GUARD: Mutex<Option<Weak<GuardInner>>> = Mutex::new(None);
static RESERVED: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Process-wide slot so panic and console-control paths can restore after the
/// owning stack frame is gone. It stores only a [`Weak`] so a dropped guard
/// does not leak, and tests observe a mock backend rather than console state.
fn lock_active() -> std::sync::MutexGuard<'static, Option<Weak<GuardInner>>> {
    ACTIVE_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
enum GuardKind {
    Native,
    Mock,
}

#[derive(Debug)]
struct GuardInner {
    kind: GuardKind,
    raw: AtomicBool,
    alternate: AtomicBool,
    cursor_hidden: AtomicBool,
    bracketed_paste: AtomicBool,
    restored: AtomicBool,
    restore_count: AtomicUsize,
}

impl GuardInner {
    fn restore(&self) {
        if self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        self.restore_count.fetch_add(1, Ordering::SeqCst);
        self.raw.store(false, Ordering::SeqCst);
        self.alternate.store(false, Ordering::SeqCst);
        self.cursor_hidden.store(false, Ordering::SeqCst);
        self.bracketed_paste.store(false, Ordering::SeqCst);
        if matches!(self.kind, GuardKind::Native) {
            let _ = execute!(stdout(), DisableBracketedPaste, Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
    }
}

/// RAII owner of terminal raw mode, alternate screen, and hidden cursor.
#[derive(Debug)]
pub struct TerminalGuard {
    inner: Arc<GuardInner>,
}

impl TerminalGuard {
    /// Enters raw mode and the alternate screen on the real terminal.
    ///
    /// Also installs a once-per-process panic hook that restores the active
    /// guard. Tests must use [`Self::new_mocked`] instead of this constructor.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when enabling raw mode or switching screens fails.
    /// Each successful stage is rolled back if a later stage fails.
    pub fn enter() -> io::Result<Self> {
        claim_active_slot()?;
        if let Err(error) = enable_raw_mode() {
            release_claimed_slot();
            return Err(error);
        }
        if let Err(error) = execute!(stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            release_claimed_slot();
            return Err(error);
        }
        if let Err(error) = execute!(stdout(), Hide) {
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            release_claimed_slot();
            return Err(error);
        }
        if let Err(error) = execute!(stdout(), EnableBracketedPaste) {
            let _ = execute!(stdout(), Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            release_claimed_slot();
            return Err(error);
        }
        install_panic_hook();
        Ok(Self::from_claimed(GuardInner {
            kind: GuardKind::Native,
            raw: AtomicBool::new(true),
            alternate: AtomicBool::new(true),
            cursor_hidden: AtomicBool::new(true),
            bracketed_paste: AtomicBool::new(true),
            restored: AtomicBool::new(false),
            restore_count: AtomicUsize::new(0),
        }))
    }

    /// Creates an entered mock guard that never talks to the console.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when another guard is already active.
    pub fn new_mocked() -> io::Result<(Self, RestoreProbe)> {
        claim_active_slot()?;
        let guard = Self::from_claimed(GuardInner {
            kind: GuardKind::Mock,
            raw: AtomicBool::new(true),
            alternate: AtomicBool::new(true),
            cursor_hidden: AtomicBool::new(true),
            bracketed_paste: AtomicBool::new(true),
            restored: AtomicBool::new(false),
            restore_count: AtomicUsize::new(0),
        });
        let probe = RestoreProbe {
            inner: Arc::clone(&guard.inner),
        };
        Ok((guard, probe))
    }

    fn from_claimed(inner: GuardInner) -> Self {
        let inner = Arc::new(inner);
        {
            let mut slot = lock_active();
            *slot = Some(Arc::downgrade(&inner));
        }
        Self { inner }
    }

    /// Restores terminal modes if this guard still owns them.
    pub fn restore(&self) {
        self.inner.restore();
    }

    /// Returns whether restore has already run.
    #[must_use]
    pub fn is_restored(&self) -> bool {
        self.inner.restored.load(Ordering::SeqCst)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.inner.restore();
        clear_active(&self.inner);
    }
}

/// Observes mock restore without holding the RAII guard.
#[derive(Debug, Clone)]
pub struct RestoreProbe {
    inner: Arc<GuardInner>,
}

impl RestoreProbe {
    /// Returns whether restore has run.
    #[must_use]
    pub fn is_restored(&self) -> bool {
        self.inner.restored.load(Ordering::SeqCst)
    }

    /// Returns how many times restore actually ran.
    #[must_use]
    pub fn restore_count(&self) -> usize {
        self.inner.restore_count.load(Ordering::SeqCst)
    }

    /// Returns whether mock raw mode is still entered.
    #[must_use]
    pub fn is_raw_mode(&self) -> bool {
        self.inner.raw.load(Ordering::SeqCst)
    }

    /// Returns whether the mock alternate screen is still entered.
    #[must_use]
    pub fn is_alternate_screen(&self) -> bool {
        self.inner.alternate.load(Ordering::SeqCst)
    }

    /// Returns whether the mock cursor is still hidden.
    #[must_use]
    pub fn is_cursor_hidden(&self) -> bool {
        self.inner.cursor_hidden.load(Ordering::SeqCst)
    }

    /// Returns whether mock bracketed paste is still entered.
    #[must_use]
    pub fn is_bracketed_paste(&self) -> bool {
        self.inner.bracketed_paste.load(Ordering::SeqCst)
    }
}

/// Restores the active guard after panic unwind or a console-control event.
///
/// On Windows, a host can call this from a console control callback. The
/// function is the same path used by the panic hook installed in
/// [`TerminalGuard::enter`].
pub fn restore_on_abnormal_exit() {
    let inner = {
        let slot = lock_active();
        slot.as_ref().and_then(Weak::upgrade)
    };
    if let Some(inner) = inner {
        inner.restore();
    }
    let mut slot = lock_active();
    *slot = None;
    RESERVED.store(false, Ordering::SeqCst);
}

fn claim_active_slot() -> io::Result<()> {
    let slot = lock_active();
    if slot.as_ref().and_then(Weak::upgrade).is_some() || RESERVED.swap(true, Ordering::SeqCst) {
        return Err(io::Error::other("a terminal guard is already active"));
    }
    Ok(())
}

fn release_claimed_slot() {
    RESERVED.store(false, Ordering::SeqCst);
}

fn clear_active(inner: &Arc<GuardInner>) {
    let mut slot = lock_active();
    let still_this = slot
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|active| Arc::ptr_eq(&active, inner));
    if still_this {
        *slot = None;
    }
    RESERVED.store(false, Ordering::SeqCst);
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore_on_abnormal_exit();
            previous(info);
        }));
    });
}
