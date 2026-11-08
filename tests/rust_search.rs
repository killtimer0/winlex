pub fn first_match(
    search: winlex_core::Result<winlex_core::Search<'_>>,
) -> winlex_core::SearchMatch {
    search_results(search).next().unwrap()
}

// Unwrap only consumed hits so predicate-based selection remains lazy.
pub fn search_results(
    search: winlex_core::Result<winlex_core::Search<'_>>,
) -> impl Iterator<Item = winlex_core::SearchMatch> {
    search.unwrap().map(Result::unwrap)
}
