use super::*;

#[test]
fn matches_ordered_unicode_subsequences_without_fabricating_a_match() {
    let mut matcher = Matcher::new("数模").unwrap();
    assert!(matcher.score("数据模型").is_some());
    assert!(matcher.score("模型数据").is_none());
    assert!(matcher.score("数据").is_none());
}

#[test]
fn exact_prefix_and_contiguous_matches_outrank_scattered_matches() {
    let mut matcher = Matcher::new("search").unwrap();
    let exact = matcher.score("search").unwrap();
    let prefix = matcher.score("search_panel").unwrap();
    let substring = matcher.score("global_search").unwrap();
    let scattered = matcher.score("some-example-archive").unwrap();
    assert!(exact > prefix);
    assert!(prefix > substring);
    assert!(substring > scattered);
}

#[test]
fn word_boundaries_and_contiguous_runs_improve_scattered_match_quality() {
    let mut matcher = Matcher::new("gsc").unwrap();
    let boundaries = matcher.score("g_s_c").unwrap();
    let scattered = matcher.score("gxxsxxc").unwrap();
    assert!(boundaries > scattered);
}

#[test]
fn case_does_not_reject_a_candidate() {
    let mut matcher = Matcher::new("Bld").unwrap();
    assert!(matcher.score("build").is_some());
    assert!(matcher.score("Build").is_some());
}

#[test]
fn path_match_prefers_the_name_when_scores_are_equal() {
    let mut matcher = Matcher::new("src").unwrap();
    let name = matcher.score_path("src", "nested/src").unwrap();
    let path = matcher.score_path("other", "src").unwrap();
    assert!(name > path);
}
