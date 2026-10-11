use super::*;

// --- Candidate preservation tests ---

#[test]
fn test_live_text_preserved_in_conversion_via_down() {
    // When DOWN is pressed during live conversion, the AI inference result
    // (live_conversion_text) should appear in the candidate list.
    let mut engine = make_live_conversion_engine();

    // Simulate typing "あい" with live conversion active
    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    set_live_text(&mut engine, "愛");

    // Press DOWN → start_conversion()
    let result = engine.process_key(&press_key(Keysym::DOWN));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    // The candidate list should contain "愛"
    let candidates = engine.state().candidates().unwrap();
    assert!(
        candidates.candidates().iter().any(|c| c.text == "愛"),
        "AI inference result '愛' should be in the candidate list"
    );
}

#[test]
fn test_live_text_not_duplicated_in_conversion() {
    // If the live_text matches the reading, it should not be duplicated
    let mut engine = make_live_conversion_engine();

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    // live_conversion_text same as hiragana reading → should not be added
    set_live_text(&mut engine, "あい");

    let result = engine.process_key(&press_key(Keysym::DOWN));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    // "あい" should not appear twice (it's same as reading, so live_text is skipped)
    let candidates = engine.state().candidates().unwrap();
    let count = candidates
        .candidates()
        .iter()
        .filter(|c| c.text == "あい")
        .count();
    assert_eq!(count, 1, "Reading should appear exactly once");
}

#[test]
fn test_suggest_result_preserved_in_start_conversion() {
    // When Space is pressed, the previous auto-suggest/live conversion result
    // should be preserved in the candidate list even if re-inference doesn't produce it.
    // (Without a kanji converter, build_conversion_candidates returns fallback only,
    // so the live_conversion_text would be lost without the preservation logic.)
    let mut engine = InputMethodEngine::new();

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    set_live_text(&mut engine, "愛");

    // Press Space → start_conversion()
    let result = engine.process_key(&press_key(Keysym::SPACE));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    // "愛" should be preserved in the candidate list
    let candidates = engine.state().candidates().unwrap();
    assert!(
        candidates.candidates().iter().any(|c| c.text == "愛"),
        "Previous suggest result '愛' should be preserved in candidates"
    );
}

#[test]
fn test_empty_live_text_not_added_to_candidates() {
    // When live_conversion_text is empty, no extra candidate should be added
    let mut engine = make_live_conversion_engine();

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    // Force empty to test the "no live text" scenario
    engine.live.shown = false;

    // DOWN → start_conversion()
    let result = engine.process_key(&press_key(Keysym::DOWN));
    assert!(result.consumed);

    // Should have candidates but no empty-string candidate
    if let Some(candidates) = engine.state().candidates() {
        assert!(
            !candidates.candidates().iter().any(|c| c.text.is_empty()),
            "Empty candidate should not be in the list"
        );
    }
}

// --- Without live conversion the window predicts, and Space converts ---

fn candidate_texts(list: &CandidateList) -> Vec<String> {
    list.candidates().iter().map(|c| c.text.clone()).collect()
}

fn live_off_engine() -> InputMethodEngine {
    InputMethodEngine::with_config(EngineConfig {
        live_conversion: false,
        ..EngineConfig::default()
    })
}

fn type_romaji(engine: &mut InputMethodEngine, romaji: &str) {
    for ch in romaji.chars() {
        engine.process_key(&press(ch));
    }
}

const AI_WORDS: &str = r#"[
    {"reading":"あい","candidates":[{"surface":"愛","score":1.0}]},
    {"reading":"あいさつ","candidates":[{"surface":"挨拶","score":1.0}]}
]"#;

#[test]
fn typing_offers_predictions_not_the_conversion_of_what_was_typed() {
    let mut engine = live_off_engine();
    engine.dicts.system = Some(dict_from_json(AI_WORDS));
    type_romaji(&mut engine, "ai");

    let shown = candidate_texts(&engine.shown_suggestions);
    assert_eq!(shown, ["挨拶"], "continuations only, not 愛 or the kana");
}

#[test]
fn space_selects_the_answer_row_past_the_kana() {
    let mut engine = live_off_engine();
    engine.dicts.system = Some(dict_from_json(AI_WORDS));
    type_romaji(&mut engine, "ai");

    let result = engine.process_key(&press_key(Keysym::SPACE));
    assert!(
        result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::ShowCandidates(_)))
    );
    let list = engine.state().candidates().unwrap();
    // Row 1 is the reading in kana, and selection starts past it.
    assert_eq!(list.candidates()[0].text, "あい");
    assert_eq!(list.cursor(), 1);
    assert_eq!(list.selected_text(), Some("愛"));
    assert_eq!(engine.preedit().unwrap().text(), "愛");

    engine.process_key(&press_key(Keysym::SPACE));
    assert_eq!(engine.state().candidates().unwrap().cursor(), 2);
}

#[test]
fn tab_selects_the_first_prediction_and_space_then_converts_what_was_typed() {
    let mut engine = live_off_engine();
    engine.dicts.system = Some(dict_from_json(AI_WORDS));
    type_romaji(&mut engine, "ai");
    let shown = candidate_texts(&engine.shown_suggestions);

    engine.process_key(&press_key(Keysym::TAB));
    assert_eq!(engine.state().stage(), ConversionStage::Predicted);
    let list = engine.state().candidates().unwrap();
    assert_eq!(candidate_texts(list), shown, "the window on screen, as is");
    assert_eq!(list.selected_text(), Some("挨拶"));

    // Space leaves the prediction for the conversion of exactly "あい".
    engine.process_key(&press_key(Keysym::SPACE));
    assert_eq!(engine.state().stage(), ConversionStage::Open);
    let list = engine.state().candidates().unwrap();
    assert_eq!(list.candidates()[0].text, "あい");
    assert_eq!(list.selected_text(), Some("愛"));
}

#[test]
fn tab_keeps_a_history_prediction_in_the_window() {
    let mut engine = engine_with_learned("あいさつ", "挨拶");
    type_romaji(&mut engine, "ai");
    engine.process_key(&press_key(Keysym::TAB));
    let list = engine.state().candidates().unwrap();
    assert_eq!(list.selected_text(), Some("挨拶"));
}

#[test]
fn tab_converts_when_no_window_is_showing() {
    let mut engine = InputMethodEngine::with_config(EngineConfig {
        candidate_window: crate::config::settings::CandidateWindow::Conversion,
        ..EngineConfig::default()
    });
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[{"surface":"愛","score":1.0}]}]"#,
    ));
    type_romaji(&mut engine, "ai");
    assert!(engine.shown_suggestions.is_empty());

    engine.process_key(&press_key(Keysym::TAB));
    let texts = candidate_texts(engine.state().candidates().expect("conversion state"));
    assert!(texts.contains(&"愛".to_string()), "{texts:?}");
}

#[test]
fn space_starts_on_the_converted_answer_when_the_top_one_stays_kana() {
    // Top-1 leaves 「おかしくて」 in kana, the beam also has 「可笑しくて」.
    // Space is a request to convert, so the kanji is what gets selected.
    let mut engine = live_off_engine();
    seed_model_cache(&mut engine, "オカシクテ", "", &["おかしくて", "可笑しくて"]);
    type_romaji(&mut engine, "okasikute");
    engine.process_key(&press_key(Keysym::SPACE));

    let list = engine.state().candidates().unwrap();
    assert_eq!(list.candidates()[0].text, "おかしくて");
    assert_eq!(list.selected_text(), Some("可笑しくて"));
}

#[test]
fn a_learned_kana_spelling_is_selected_rather_than_skipped() {
    // The user once committed 「おかしくて」 as it is: that is the answer.
    let mut engine = engine_with_learned("おかしくて", "おかしくて");
    type_romaji(&mut engine, "okasikute");
    engine.process_key(&press_key(Keysym::SPACE));
    assert_eq!(
        engine.state().candidates().unwrap().selected_text(),
        Some("おかしくて")
    );
}

// --- The two-part window (macOS): predictions above, Space's rows below ---

fn split_engine() -> InputMethodEngine {
    let mut engine = InputMethodEngine::with_config(EngineConfig {
        live_conversion: false,
        split_window: true,
        ..EngineConfig::default()
    });
    engine.dicts.system = Some(dict_from_json(AI_WORDS));
    engine
}

#[test]
fn split_window_holds_predictions_above_and_space_rows_below() {
    let mut engine = split_engine();
    type_romaji(&mut engine, "ai");

    let list = &engine.shown_suggestions;
    let texts = candidate_texts(list);
    assert_eq!(list.divider(), Some(1));
    assert_eq!(texts[0], "挨拶", "the prediction leads");
    // Below the divider: the very rows Space opens on, kana row first.
    assert_eq!(&texts[1..3], ["あい", "愛"], "texts were: {texts:?}");
    assert_eq!(list.page_divider(), Some(1));
    assert!(!list.has_selection(), "Enter still commits what was typed");
    assert!(texts.len() <= CandidateList::DEFAULT_PAGE_SIZE);
}

#[test]
fn split_window_without_predictions_highlights_the_answer_space_selects() {
    let mut engine = split_engine();
    type_romaji(&mut engine, "okasikute");
    let list = &engine.shown_suggestions;
    assert_eq!(list.divider(), Some(0));
    assert_eq!(list.page_divider(), None, "nothing to separate");
    assert_eq!(list.candidates()[0].text, "おかしくて");
    assert!(!list.has_selection());
}

#[test]
fn split_window_tab_enters_the_prediction_rows_and_space_converts_exactly() {
    let mut engine = split_engine();
    type_romaji(&mut engine, "ai");
    engine.process_key(&press_key(Keysym::TAB));
    assert_eq!(engine.state().stage(), ConversionStage::Predicted);
    let list = engine.state().candidates().unwrap();
    assert_eq!(list.selected_text(), Some("挨拶"));
    assert_eq!(list.divider(), Some(1), "the window keeps its two parts");
    assert!(list.has_selection());

    engine.process_key(&press_key(Keysym::SPACE));
    let list = engine.state().candidates().unwrap();
    assert_eq!(list.divider(), None, "now the plain conversion list");
    assert_eq!(list.selected_text(), Some("愛"));
}

#[test]
fn split_window_space_leaves_the_predictions_behind() {
    let mut engine = split_engine();
    type_romaji(&mut engine, "ai");
    engine.process_key(&press_key(Keysym::SPACE));
    let list = engine.state().candidates().unwrap();
    assert_eq!(list.divider(), None);
    assert_eq!(list.selected_text(), Some("愛"));
}

#[test]
fn split_window_stays_off_without_a_frontend_that_draws_it() {
    let mut engine = live_off_engine();
    engine.dicts.system = Some(dict_from_json(AI_WORDS));
    type_romaji(&mut engine, "ai");
    assert_eq!(engine.shown_suggestions.divider(), None);
}

#[test]
fn predictions_hold_five_continuations_even_when_the_typed_reading_is_learned() {
    // 「よろ」 itself is in the history (exact match), plus six continuations.
    let mut engine = live_off_engine();
    let mut cache = LearningCache::new(LearningConfig::default());
    for (reading, surface) in [
        ("よろ", "よろ"),
        ("よろしく", "よろしく"),
        ("よろしくおねがいします", "よろしくお願いします"),
        ("よろしくおねがいたします", "よろしくお願いたします"),
        ("よろしくたのむ", "よろしく頼む"),
        ("よろこび", "喜び"),
        ("よろず", "万"),
    ] {
        cache.record(reading, surface);
    }
    engine.learning = Some(cache);
    type_romaji(&mut engine, "yoro");

    let texts = candidate_texts(&engine.shown_suggestions);
    assert_eq!(texts.len(), 5, "texts were: {texts:?}");
    assert!(
        !texts.contains(&"よろ".to_string()),
        "the typed reading is not a prediction"
    );
    assert!(texts.contains(&"よろしくお願いします".to_string()));
}

#[test]
fn split_window_enter_commits_the_typed_reading_and_tab_walks_on_below() {
    let mut engine = split_engine();
    type_romaji(&mut engine, "ai");
    // Tab past the one prediction lands on the rows Space would have opened.
    engine.process_key(&press_key(Keysym::TAB));
    engine.process_key(&press_key(Keysym::TAB));
    assert_eq!(
        engine.state().candidates().unwrap().selected_text(),
        Some("あい")
    );

    let mut engine = split_engine();
    type_romaji(&mut engine, "ai");
    let result = engine.process_key(&press_key(Keysym::RETURN));
    assert!(
        result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::Commit(t) if t == "あい"))
    );
}
