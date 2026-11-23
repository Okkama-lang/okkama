//! The regex input limit Sparrow's `runtime/regex` provider
//! (`host/native_regex.rs`) shares with this crate: `MAX_INPUT` and
//! `input_scalars`. The regex engine itself is Sparrow's; no `runtime/regex`
//! native reaches `native::execute`.

/// The regex input limit in Unicode scalars, shared with Sparrow's engine.
pub const MAX_INPUT: usize = 1_048_576;

/// How many scalars `chars` yields, or `None` past `limit`: it reads at most
/// `limit + 1` of them and stores none (#961).
pub fn bounded_scalar_count(chars: impl Iterator<Item = char>, limit: usize) -> Option<usize> {
    let count = chars.take(limit.saturating_add(1)).count();
    (count <= limit).then_some(count)
}

/// The scalars of a regex input within `limit`: counted before anything is
/// stored, so an input past the limit is refused with no storage, and an
/// admitted one is stored once at its exact size (#961). Sparrow's regex
/// provider (`host/native_regex.rs`) prepares its input here.
pub fn input_scalars(input: &str, limit: usize) -> Result<Vec<u32>, &'static str> {
    let Some(count) = bounded_scalar_count(input.chars(), limit) else {
        return Err("Regex input resource limit exceeded\n");
    };
    let mut points = Vec::with_capacity(count);
    points.extend(input.chars().map(u32::from));
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #961: at the scalar limit an input is admitted and stored at its exact
    /// size; one scalar beyond it is refused with the same diagnostic, for
    /// ASCII and for 2- and 4-byte scalars.
    #[test]
    fn regex_input_is_admitted_at_the_limit_and_refused_one_beyond() {
        for scalar in ['a', '\u{e9}', '\u{1d11e}'] {
            let at: String = std::iter::repeat_n(scalar, MAX_INPUT).collect();
            let points = input_scalars(&at, MAX_INPUT).expect("at the limit");
            assert_eq!(points.len(), MAX_INPUT, "{scalar:?}");
            assert_eq!(points.capacity(), MAX_INPUT, "{scalar:?}: stored at its exact size");
            assert!(points.iter().all(|&point| point == u32::from(scalar)), "{scalar:?}");
            let beyond = format!("{at}{scalar}");
            assert_eq!(input_scalars(&beyond, MAX_INPUT), Err("Regex input resource limit exceeded\n"), "{scalar:?}");
        }
    }

    /// #961: refusing an input reads at most `limit + 1` scalars and stores
    /// none, however long the input is; an admitted input is read whole.
    #[test]
    fn regex_input_refusal_reads_a_bounded_prefix() {
        let pulled = std::cell::Cell::new(0usize);
        let counted = |text: &'static str| {
            pulled.set(0);
            text.chars().inspect(|_| pulled.set(pulled.get() + 1))
        };
        let long: &'static str = Box::leak("\u{e9}".repeat(10_000).into_boxed_str());
        assert_eq!(bounded_scalar_count(counted(long), 4), None);
        assert_eq!(pulled.get(), 5, "a refusal stops after limit + 1 scalars");
        assert_eq!(bounded_scalar_count(counted("ab\u{e9}d"), 4), Some(4));
        assert_eq!(pulled.get(), 4);
        assert_eq!(bounded_scalar_count(counted(""), 0), Some(0));
        assert_eq!(input_scalars("abcde", 4), Err("Regex input resource limit exceeded\n"));
        let small = input_scalars("ab\u{e9}", 4).expect("within the limit");
        assert_eq!((small.len(), small.capacity()), (3, 3));
    }
}
