//! Test support shared by every task: counted checks, budgeted sections that
//! survive panics and hangs, a counting global allocator and a seeded RNG.
//!
//! Output protocol (parsed by `bench_core`):
//!   BENCH-BUDGET <n>                       once, before any section
//!   FAIL <file>:<line> / "  check: ..."    one block per failed check
//!   BENCH-SECTION <name> <pass> <fail> <budget>
//!   BENCH-TOTAL <pass> <fail> <budget>     last line; absent if the process died
//!
//! A section that panics or times out fails its whole unfinished budget, and
//! every finished section is already on stdout, so a later crash keeps the
//! credit that was earned before it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- allocator

static LIVE: AtomicIsize = AtomicIsize::new(0);
static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BAD_FREES: AtomicUsize = AtomicUsize::new(0);

/// Install with `#[global_allocator] static A: bench_util::CountingAlloc = bench_util::CountingAlloc;`
pub struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(1, SeqCst);
            LIVE_BYTES.fetch_add(l.size() as isize, SeqCst);
            ALLOCS.fetch_add(1, SeqCst);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            LIVE.fetch_add(1, SeqCst);
            LIVE_BYTES.fetch_add(l.size() as isize, SeqCst);
            ALLOCS.fetch_add(1, SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if LIVE.fetch_sub(1, SeqCst) <= 0 {
            BAD_FREES.fetch_add(1, SeqCst);
        }
        LIVE_BYTES.fetch_sub(l.size() as isize, SeqCst);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            LIVE_BYTES.fetch_add(n as isize - l.size() as isize, SeqCst);
            ALLOCS.fetch_add(1, SeqCst);
        }
        q
    }
}

/// Live heap blocks right now (needs `CountingAlloc` installed).
pub fn live_allocs() -> isize {
    LIVE.load(SeqCst)
}
pub fn live_bytes() -> isize {
    LIVE_BYTES.load(SeqCst)
}
/// Total allocations made so far.
pub fn total_allocs() -> usize {
    ALLOCS.load(SeqCst)
}
/// Frees that had no matching live allocation.
pub fn bad_frees() -> usize {
    BAD_FREES.load(SeqCst)
}

/// Run `f` and report how many heap blocks it left alive.
pub fn leaked_by<R>(f: impl FnOnce() -> R) -> (R, isize) {
    let before = live_allocs();
    let r = f();
    (r, live_allocs() - before)
}

// ---------------------------------------------------------------------- rng

/// xorshift64*: small, fast, reproducible.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// Seeded from `BENCH_SEED` and a tag, so each test gets its own stream.
    pub fn for_tag(tag: &str) -> Self {
        let mut h = seed() ^ 0xCBF2_9CE4_8422_2325;
        for b in tag.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01B3);
        }
        Rng::new(h)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i as u64 + 1) as usize);
        }
    }
    pub fn ascii_string(&mut self, alphabet: &str, len: usize) -> String {
        let a: Vec<char> = alphabet.chars().collect();
        (0..len).map(|_| a[self.below(a.len() as u64) as usize]).collect()
    }
}

/// The run's seed: `BENCH_SEED` if set, otherwise a fixed constant.
pub fn seed() -> u64 {
    static S: OnceLock<u64> = OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("BENCH_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0xC0FFEE)
    })
}

/// Time a closure.
pub fn timed<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

// ------------------------------------------------------------------- checks

struct Ctx {
    pass: AtomicUsize,
    fail: AtomicUsize,
    shown: AtomicUsize,
}

thread_local! {
    static LOCAL: RefCell<Option<Arc<Ctx>>> = const { RefCell::new(None) };
    static PANIC_MSG: RefCell<String> = const { RefCell::new(String::new()) };
}
static CURRENT: Mutex<Option<Arc<Ctx>>> = Mutex::new(None);
static OUT: Mutex<()> = Mutex::new(());
static TOTALS: Mutex<(usize, usize, usize)> = Mutex::new((0, 0, 0));
static DECLARED: AtomicUsize = AtomicUsize::new(0);
const MAX_SHOWN: usize = 8;

fn emit(s: &str) {
    let _g = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}

fn ctx() -> Option<Arc<Ctx>> {
    LOCAL
        .with(|l| l.borrow().clone())
        .or_else(|| CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

#[doc(hidden)]
pub fn record(ok: bool, file: &str, line: u32, expr: &str, detail: impl FnOnce() -> String) {
    let Some(c) = ctx() else { return };
    if ok {
        c.pass.fetch_add(1, SeqCst);
        return;
    }
    c.fail.fetch_add(1, SeqCst);
    if c.shown.fetch_add(1, SeqCst) < MAX_SHOWN {
        let d = detail();
        let mut s = format!("FAIL {file}:{line}\n  check: {expr}");
        if !d.is_empty() {
            s.push_str("\n  ");
            s.push_str(&d);
        }
        emit(&s);
    }
}

/// `check!(cond)` or `check!(cond, "context {}", x)`.
#[macro_export]
macro_rules! check {
    ($c:expr) => { $crate::record($c, file!(), line!(), stringify!($c), String::new) };
    ($c:expr, $($arg:tt)+) => { $crate::record($c, file!(), line!(), stringify!($c), || format!($($arg)+)) };
}

/// `check_eq!(actual, expected)`: prints both on failure.
/// `check_eq!(actual, expected, "context {}", x)` adds a line of context.
#[macro_export]
macro_rules! check_eq {
    ($a:expr, $b:expr) => {
        // A match scrutinee keeps its temporaries alive for the whole arm, so
        // `check_eq!(f().as_deref(), ..)` does not hit E0716. A tuple pattern
        // in a let does not, and that silently made some visible suites
        // unbuildable for every implementation that returned an owned value.
        match (&$a, &$b) {
            (l, r) => $crate::record(*l == *r, file!(), line!(), concat!(stringify!($a), " == ", stringify!($b)),
                || format!("left: {:?}\n  right: {:?}", l, r)),
        }
    };
    ($a:expr, $b:expr, $($arg:tt)+) => {
        match (&$a, &$b) {
            (l, r) => $crate::record(*l == *r, file!(), line!(), concat!(stringify!($a), " == ", stringify!($b)),
                || format!("{}\n  left: {:?}\n  right: {:?}", format!($($arg)+), l, r)),
        }
    };
}

fn install_hook() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        panic::set_hook(Box::new(|info| {
            let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic>".into()
            };
            PANIC_MSG.with(|m| *m.borrow_mut() = format!("{msg} at {loc}"));
        }));
    });
}

/// Declare the task's total check budget. Call once, first.
pub fn budget(total: usize) {
    install_hook();
    DECLARED.store(total, SeqCst);
    emit(&format!("BENCH-BUDGET {total}"));
}

/// Run one named group of checks. `budget` is the exact number of checks the
/// group performs when the implementation is correct. A panic or a run past
/// `secs` fails every check that had not run yet.
///
/// The body runs on its own 8 MiB-stack thread, which is joined before the next
/// section starts. A section that panics or times out fails its unfinished
/// budget and the suite carries on.
pub fn section<F: FnOnce() + Send + 'static>(name: &str, budget: usize, secs: u64, f: F) {
    install_hook();
    let c = Arc::new(Ctx { pass: AtomicUsize::new(0), fail: AtomicUsize::new(0), shown: AtomicUsize::new(0) });
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let c2 = c.clone();
    let spawned = std::thread::Builder::new().stack_size(8 << 20).spawn(move || {
        LOCAL.with(|l| *l.borrow_mut() = Some(c2));
        let r = panic::catch_unwind(AssertUnwindSafe(f));
        let msg = r.err().map(|_| PANIC_MSG.with(|m| m.borrow().clone()));
        let _ = tx.send(msg);
    });
    let mut abnormal = false;
    let mut done = None;
    match spawned {
        Err(_) => {
            abnormal = true;
            emit(&format!("FAIL section {name}: could not start"));
        }
        Ok(handle) => match rx.recv_timeout(Duration::from_secs(secs.max(1))) {
            Ok(None) => done = Some(handle),
            Ok(Some(msg)) => {
                abnormal = true;
                emit(&format!("FAIL section {name}: panicked: {msg}"));
                done = Some(handle);
            }
            // A section that ran past its limit is left detached: there is no way
            // to stop it, and blocking here would defeat the limit. It can still
            // perturb a later section that measures the process-wide allocator
            // counters, which is why timeouts are a real failure, not a warning.
            Err(_) => {
                abnormal = true;
                emit(&format!("FAIL section {name}: timed out after {secs}s"));
            }
        },
    }
    // A section that finished is joined before anything else looks at the
    // global allocator counters: its own thread still has to free its bookkeeping,
    // and `leaked_by` in the next section must not see that as a live block.
    if let Some(handle) = done {
        let _ = handle.join();
    }
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let (mut pass, mut fail) = (c.pass.load(SeqCst), c.fail.load(SeqCst));
    let ran = pass + fail;
    if std::env::var_os("BENCH_LEARN").is_some() {
        // Maintainer mode: report exactly what ran so budgets can be rewritten.
        emit(&format!("BENCH-SECTION {name} {pass} {fail} {ran}"));
        let mut t = TOTALS.lock().unwrap_or_else(|e| e.into_inner());
        t.0 += pass;
        t.1 += fail;
        t.2 += ran;
        return;
    }
    if abnormal {
        fail += budget.saturating_sub(ran);
    } else if ran != budget {
        emit(&format!("BUDGET MISMATCH section {name}: ran {ran}, expected {budget}"));
        if ran < budget {
            fail += budget - ran;
        } else {
            // Extra checks cannot be earned: cap the pass count at the budget.
            pass = pass.min(budget);
            fail = fail.max(1);
        }
    }
    {
        let mut t = TOTALS.lock().unwrap_or_else(|e| e.into_inner());
        t.0 += pass;
        t.1 += fail;
        t.2 += budget;
    }
    emit(&format!("BENCH-SECTION {name} {pass} {fail} {budget}"));
}

/// Print the totals and exit (0 only for a perfect, fully-budgeted run).
pub fn finish() -> ! {
    let (p, f, b) = *TOTALS.lock().unwrap_or_else(|e| e.into_inner());
    let declared = DECLARED.load(SeqCst);
    if declared != b {
        emit(&format!("BUDGET MISMATCH total: sections {b}, declared {declared}"));
    }
    emit(&format!("BENCH-TOTAL {p} {f} {b}"));
    std::process::exit(if f == 0 && declared == b { 0 } else { 1 });
}

// ------------------------------------------------------------ drop tracking

/// Records the id of every `Tracked` value that is dropped, to catch leaks
/// (never dropped) and double drops (dropped twice) in generic containers.
#[derive(Clone, Default)]
pub struct DropLog(Arc<Mutex<Vec<u32>>>);

impl DropLog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn track(&self, id: u32) -> Tracked {
        Tracked { id, log: self.clone() }
    }
    /// Ids dropped so far, sorted.
    pub fn dropped(&self) -> Vec<u32> {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v.sort_unstable();
        v
    }
    pub fn count(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    /// True when exactly the ids in `want` were dropped, each once.
    pub fn dropped_exactly(&self, want: impl IntoIterator<Item = u32>) -> bool {
        let mut w: Vec<u32> = want.into_iter().collect();
        w.sort_unstable();
        self.dropped() == w
    }
}

pub struct Tracked {
    pub id: u32,
    log: DropLog,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.log.0.lock().unwrap_or_else(|e| e.into_inner()).push(self.id);
    }
}
impl PartialEq for Tracked {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}
impl Eq for Tracked {}
impl PartialOrd for Tracked {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Tracked {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.id.cmp(&o.id)
    }
}
impl std::fmt::Debug for Tracked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tracked({})", self.id)
    }
}
