//! Opt-in test oracle. The original dense domain and accounting are kept in
//! precise_linearity; production builds cannot select that solver.
use super::*;
use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Mutex;

static RECORD_LOCK: Mutex<()> = Mutex::new(());

fn append_record(variable: &str, record: &str) {
    if let Some(path) = std::env::var_os(variable) {
        let _guard = RECORD_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        file.write_all(record.as_bytes()).unwrap();
    }
}

fn verdict(result: &Result<VerifiedModule<'_>, VerifyReport>) -> Option<VerifyFailure> {
    result.as_ref().err().map(|report| report.failure)
}
fn diagnostics(result: &Result<VerifiedModule<'_>, VerifyReport>) -> BTreeSet<(RuleId, EntityLocation)> {
    result.as_ref().err().into_iter().flat_map(|report| &report.diagnostics)
        .map(|diagnostic| (diagnostic.rule, diagnostic.primary.entity.clone())).collect()
}
fn stats<'a>(result: &'a Result<VerifiedModule<'_>, VerifyReport>) -> &'a VerifyStats {
    match result {
        Ok(verified) => &verified.stats,
        Err(report) => &report.stats,
    }
}
pub(super) fn compare(
    module: &Module,
    options: &VerifyOptions,
    context: VerifyContext<'_>,
    profile: Option<Profile>,
    function: Option<FuncId>,
    candidate: &Result<VerifiedModule<'_>, VerifyReport>,
) {
    // Budget tests exercise representation-specific allocation charges. Their
    // compact assertions still run, but only default limits define the oracle.
    if options.limits != VerifyLimits::default() {
        return;
    }
    for errors in [ErrorPolicy::All, ErrorPolicy::First] {
        let other;
        let compact = if errors == options.errors {
            candidate
        } else {
            let mut alternate = options.clone();
            alternate.errors = errors;
            other = run_mode(module, &alternate, context, profile, function, false);
            &other
        };
        let mut precise_options = options.clone();
        precise_options.errors = errors;
        let dense = run_mode(module, &precise_options, context, profile, function, true);
        let complete = verdict(&dense) != Some(VerifyFailure::Incomplete);
        let same_verdict = verdict(compact) == verdict(&dense);
        let same_diagnostics = diagnostics(compact) == diagnostics(&dense);
        let same_first = errors != ErrorPolicy::First
            || compact.as_ref().err().map(|report| report.diagnostics.first())
                == dense.as_ref().err().map(|report| report.diagnostics.first());
        let mismatch = complete && !(same_verdict && same_diagnostics && same_first);
        let compact_stats = stats(compact);
        let dense_stats = stats(&dense);
        let thread = std::thread::current();
        let test = thread.name().unwrap_or("unnamed");
        let record = format!(
            "{test}\t{:?}\t{function:?}\t{errors:?}\t{complete}\t{:?}\t{:?}\t{}\t{}\t{}\t{}\t{mismatch}\n",
            module.profile, verdict(&dense), verdict(compact), dense_stats.memory_bytes,
            compact_stats.memory_bytes, dense_stats.steps, compact_stats.steps,
        );
        append_record("OKKAMA_634_RESULTS", &record);
        assert!(!mismatch, "#634 differential mismatch: {record}");
        if !complete && verdict(compact) != Some(VerifyFailure::Incomplete) {
            // Only the oracle gets extra headroom. Production limits stay fixed.
            // Low P1 exceeds 8x dense memory before its fixed point; allow 64x.
            precise_options.limits.steps = precise_options.limits.steps.saturating_mul(20);
            precise_options.limits.memory_bytes = precise_options.limits.memory_bytes.saturating_mul(64);
            let raised = run_mode(module, &precise_options, context, profile, function, true);
            let confirmed = verdict(&raised) != Some(VerifyFailure::Incomplete)
                && verdict(compact) == verdict(&raised)
                && diagnostics(compact) == diagnostics(&raised)
                && (errors != ErrorPolicy::First
                    || compact.as_ref().err().map(|report| report.diagnostics.first())
                        == raised.as_ref().err().map(|report| report.diagnostics.first()));
            let raised_stats = stats(&raised);
            let confirmation = format!(
                "{test}\t{:?}\t{function:?}\t{errors:?}\t{:?}\t{}\t{}\t{confirmed}\n",
                module.profile, verdict(&raised), raised_stats.memory_bytes, raised_stats.steps,
            );
            append_record("OKKAMA_634_RAISED", &confirmation);
            assert!(confirmed, "#634 raised-limit oracle mismatch: {record}{confirmation}");
        }
    }
}

pub(super) fn record_function(
    profile: Profile,
    symbol: &str,
    precise: bool,
    before: (usize, usize),
    stats: &VerifyStats,
    scratch: usize,
    incomplete: bool,
) {
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("unnamed");
    let record = format!(
        "{test}\t{profile:?}\t{symbol}\t{precise}\t{}\t{}\t{scratch}\t{incomplete}\n",
        stats.memory_bytes - before.0, stats.steps - before.1,
    );
    append_record("OKKAMA_634_FUNCTIONS", &record);
    let end = format!("{test}\t{profile:?}\t{symbol}\tend\t{}\t{}\t{scratch}\t{incomplete}\n",
        stats.memory_bytes - before.0, stats.steps - before.1,
    );
    append_record("OKKAMA_634_RETAINED", &end);
}

pub(super) fn record_limit(
    profile: Profile,
    function: Option<FuncId>,
    errors: ErrorPolicy,
    precise: bool,
    stats: &VerifyStats,
    limit: Option<&str>,
) {
    let record = format!("{profile:?}\t{function:?}\t{errors:?}\t{precise}\t{}\t{}\t{limit:?}\n",
        stats.memory_bytes, stats.steps,
    );
    append_record("OKKAMA_634_LIMITS", &record);
}

pub(super) fn record_schedule(visits: usize, alternatives: usize, comparisons: usize) {
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("unnamed");
    let record = format!("{test}\t{visits}\t{alternatives}\t{comparisons}\n");
    append_record("OKKAMA_634_SCHEDULE", &record);
}

pub(super) fn record_retained(
    profile: Profile, symbol: &str, bytes: usize, shared: usize, nodes: usize, logical: usize,
) {
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("unnamed");
    let record = format!("{test}\t{profile:?}\t{symbol}\t{bytes}\t{shared}\t{nodes}\t{logical}\n");
    append_record("OKKAMA_634_RETAINED", &record);
}

pub(super) fn record_allocations(parts: &[usize]) {
    if std::env::var_os("OKKAMA_634_RELATIONS").is_some() {
        append_record("OKKAMA_634_RELATIONS", &format!("{parts:?}\n"));
    }
}
