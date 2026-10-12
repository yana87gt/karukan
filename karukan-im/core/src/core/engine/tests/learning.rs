//! Tests for the learning cache: candidates it surfaces and
//! Ctrl+Delete, which deletes the selected learning candidate from the history.

use super::*;
use crate::core::engine::conversion::rank_learned_by_context;
use crate::core::engine::display::LEARNING_DELETE_HINT;

#[test]
fn build_candidates_include_learning() {
    let mut engine = engine_with_learned("あい", "藍");

    let texts: Vec<String> = engine
        .build_conversion_candidates("あい", "あい", "", 9)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect();

    assert!(
        texts.contains(&"藍".to_string()),
        "conversion should surface learned `藍`, got {:?}",
        texts,
    );
}

#[test]
fn ctrl_delete_removes_selected_learning_entry() {
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    // The kana row leads, then the learning candidates (force-pushed ahead
    // of everything else); the initial selection is the first answer.
    let selected = engine
        .state()
        .candidates()
        .unwrap()
        .selected()
        .unwrap()
        .clone();
    assert_eq!(selected.text, "藍");
    assert!(
        selected.is_deletable(),
        "learning candidate must be flagged"
    );

    let result = engine.process_key(&press_ctrl(Keysym::DELETE));
    assert!(result.consumed);
    // The entry is gone from the cache...
    assert!(engine.learning.as_ref().unwrap().lookup("あい").is_empty());
    // ...and the window stays up: the conversion is rebuilt in place,
    // staying in Conversion.
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    assert!(
        result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::ShowCandidates(_))),
        "deletion must refresh the candidate window, not close it"
    );
    assert!(
        !result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::HideCandidates)),
        "deletion must not hide the candidate window"
    );

    // `藍` is no longer a *learning* candidate. It may still return from the
    // model or dictionary (deleting history doesn't blacklist a surface — the
    // whole point of rebuilding instead of dropping the row), but never again
    // flagged as user history.
    let candidates = engine.state().candidates().unwrap();
    assert!(
        !candidates
            .candidates()
            .iter()
            .any(|c| c.text == "藍" && c.is_deletable()),
        "`藍` must no longer be a learning candidate after deletion",
    );
    // The rebuilt list keeps the selection where it was: index 1, past the
    // kana row that leads the list.
    assert_eq!(candidates.cursor(), 1);
}

#[test]
fn ctrl_delete_removes_prefix_twins_so_surface_does_not_resurface() {
    // The same surface learned under two prefix-related readings is shown as a
    // single deduped row; deleting it must clear both, or the twin under the
    // longer reading pops back on the next conversion of the same input.
    let mut engine = engine_with_learned("あい", "藍");
    engine.learning.as_mut().unwrap().record("あいさ", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));

    let selected = engine
        .state()
        .candidates()
        .unwrap()
        .selected()
        .unwrap()
        .clone();
    assert_eq!(selected.text, "藍");
    assert!(selected.is_deletable());

    engine.process_key(&press_ctrl(Keysym::DELETE));
    // Both the exact and the prefix entry are gone.
    assert!(engine.learning.as_ref().unwrap().lookup("あい").is_empty());
    assert!(
        engine
            .learning
            .as_ref()
            .unwrap()
            .lookup("あいさ")
            .is_empty(),
        "the prefix twin (あいさ→藍) must be cleared too, not just the exact entry",
    );
}

#[test]
fn ctrl_delete_keeps_surface_that_another_source_also_produces() {
    // #2 regression: the learned surface equals the hiragana reading, which
    // the fallback ALWAYS produces. That fallback copy is deduped away under
    // the learning entry; deleting the entry must bring it back (now
    // non-learning) rather than remove the only row — which is why deletion
    // rebuilds the conversion instead of dropping the candidate in place.
    let mut engine = engine_with_learned("あい", "あい");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    // Space starts past the kana row, learned or not; step back onto it.
    engine.process_key(&press_key(Keysym::SPACE));
    engine.process_key(&press_key(Keysym::UP));

    let selected = engine
        .state()
        .candidates()
        .unwrap()
        .selected()
        .unwrap()
        .clone();
    assert_eq!(selected.text, "あい");
    assert!(selected.is_deletable());

    engine.process_key(&press_ctrl(Keysym::DELETE));
    assert!(engine.learning.as_ref().unwrap().lookup("あい").is_empty());

    // `あい` survives as an ordinary fallback candidate.
    let candidates = engine.state().candidates().unwrap();
    let ai = candidates.candidates().iter().find(|c| c.text == "あい");
    assert!(
        ai.is_some(),
        "the fallback `あい` must survive the deletion, not vanish with the \
         learning entry",
    );
    assert!(
        !ai.unwrap().is_deletable(),
        "the surviving `あい` must no longer be flagged as learning",
    );
}

#[test]
fn init_learning_cache_applies_configured_surface_cap() {
    // Guards the config→cache seam: if init_learning_cache stops applying
    // max_surface_chars, the 6-char surface (over the configured 5, under
    // the default 50) gets recorded and this fails.
    // A history file of the test's own: the user's must not be read.
    let dir = tempfile::tempdir().unwrap();
    let mut engine = InputMethodEngine::new();
    engine.init_learning_cache_from(
        Some(dir.path().join("learning.tsv")),
        true,
        LearningConfig {
            max_entries: 10_000,
            max_surface_chars: 5,
        },
    );
    let cache = engine.learning.as_mut().expect("learning enabled");

    let before = cache.entry_count();
    cache.record("__karukan_seam_test__", &"漢".repeat(6));
    assert_eq!(
        cache.entry_count(),
        before,
        "a surface over the configured cap must be skipped; the configured \
         value is not reaching the cache",
    );
}

#[test]
fn ctrl_backspace_deletes_learning_entry_like_ctrl_delete() {
    // Mac keyboards label the Backspace key "delete", so the natural macOS
    // chord is Ctrl+delete = Ctrl+Backspace; it must behave like Ctrl+Delete
    // (forward delete), not like the plain-Backspace cancel.
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));

    let result = engine.process_key(&press_ctrl(Keysym::BACKSPACE));
    assert!(result.consumed);
    assert!(engine.learning.as_ref().unwrap().lookup("あい").is_empty());
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
}

#[test]
fn ctrl_backspace_does_nothing_for_non_learning_candidate() {
    // When the selection isn't a learning candidate, Ctrl+Backspace (like
    // Ctrl+Delete) is consumed so it can't leak to the app mid-conversion,
    // but the conversion is left intact. Cancelling stays on plain
    // Backspace / Escape.
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    // Move the selection off the learning candidate.
    engine.process_key(&press_key(Keysym::SPACE));
    let before = engine.state().candidates().unwrap().clone();
    assert!(!before.selected().unwrap().is_deletable());

    let result = engine.process_key(&press_ctrl(Keysym::BACKSPACE));
    assert!(
        result.consumed,
        "the chord must be consumed, not leak to the app"
    );
    assert!(
        matches!(engine.state(), InputState::Conversion { .. }),
        "Ctrl+Backspace must not cancel when the selection isn't deletable"
    );
    // Nothing changed: same selection, same list, history intact.
    let after = engine.state().candidates().unwrap();
    assert_eq!(after.cursor(), before.cursor());
    assert_eq!(after.selected_text(), before.selected_text());
    assert!(!engine.learning.as_ref().unwrap().lookup("あい").is_empty());
}

#[test]
fn ctrl_backspace_in_composing_deletes_char_not_history() {
    // History deletion is a Conversion-state-only chord. During Composing,
    // Ctrl+Backspace edits text like plain Backspace, even while a learning
    // suggestion is on screen.
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    assert!(matches!(engine.state(), InputState::Composing { .. }));

    let result = engine.process_key(&press_ctrl(Keysym::BACKSPACE));
    assert!(result.consumed);
    assert_eq!(engine.input_buf.reading(), "あ");
    assert!(
        !engine.learning.as_ref().unwrap().lookup("あい").is_empty(),
        "the learning entry must survive — deletion only works in Conversion",
    );
}

#[test]
fn ctrl_alt_delete_leaves_history_alone() {
    // Ctrl+Alt+Delete is a desktop chord, not ours. The delete arm guards on
    // `!alt_key` like its siblings, so the key passes through untouched
    // instead of irreversibly purging a history entry.
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(
        engine
            .state()
            .candidates()
            .unwrap()
            .selected()
            .unwrap()
            .is_deletable()
    );

    let result = engine.process_key(&press_ctrl_alt(Keysym::DELETE));
    assert!(!result.consumed, "Ctrl+Alt+Delete must reach the desktop");
    assert!(
        !engine.learning.as_ref().unwrap().lookup("あい").is_empty(),
        "Ctrl+Alt+Delete must not delete the learning entry"
    );
}

#[test]
fn plain_backspace_still_cancels_conversion() {
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));

    let result = engine.process_key(&press_key(Keysym::BACKSPACE));
    assert!(result.consumed);
    // Backspace without Ctrl keeps its cancel-to-composing behavior and
    // deletes nothing from the history.
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert!(!engine.learning.as_ref().unwrap().lookup("あい").is_empty());
}

#[test]
fn ctrl_delete_ignores_non_learning_candidate() {
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));

    // Move the selection off the learning candidate onto a fallback one.
    engine.process_key(&press_key(Keysym::SPACE));
    let selected = engine
        .state()
        .candidates()
        .unwrap()
        .selected()
        .unwrap()
        .clone();
    assert!(!selected.is_deletable());

    let before_len = engine.state().candidates().unwrap().len();
    let result = engine.process_key(&press_ctrl(Keysym::DELETE));
    // The key is consumed (it must not leak to the app mid-conversion) but
    // nothing is deleted and the conversion continues.
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    assert_eq!(engine.state().candidates().unwrap().len(), before_len);
    assert!(!engine.learning.as_ref().unwrap().lookup("あい").is_empty());
}

#[test]
fn prefix_learning_is_offered_below_the_typed_reading() {
    // A history entry longer than what was typed still appears, but the
    // highlight is a candidate for the typed reading itself.
    let mut engine = engine_with_learned("あいさつ", "挨拶");
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[{"surface":"愛","score":1.0}]}]"#,
    ));

    let texts: Vec<String> = engine
        .build_conversion_candidates("あい", "あい", "", 9)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect();
    let exact = texts
        .iter()
        .position(|t| t == "愛")
        .expect("exact dict hit");
    let predicted = texts
        .iter()
        .position(|t| t == "挨拶")
        .expect("prefix learning hit");
    assert!(
        exact < predicted,
        "the typed reading's candidate must outrank the prediction, got {texts:?}"
    );
    assert_ne!(texts[0], "挨拶");
}

#[test]
fn exact_learning_still_heads_the_list_ahead_of_a_prediction() {
    let mut engine = engine_with_learned("あい", "藍");
    engine.learning.as_mut().unwrap().record("あいさつ", "挨拶");

    let texts: Vec<String> = engine
        .build_conversion_candidates("あい", "あい", "", 9)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect();
    // The reading in kana leads the list; the exact learning entry is the
    // first answer after it, ahead of the prediction.
    assert_eq!(texts[0], "あい");
    assert_eq!(texts[1], "藍");
    let predicted = texts.iter().position(|t| t == "挨拶").expect("prediction");
    assert!(predicted > 1);
}

#[test]
fn ctrl_delete_removes_prefix_matched_entry_by_full_reading() {
    // A prefix-matched learning candidate carries its own (longer) reading;
    // deletion must remove the cache entry under that full reading.
    let mut engine = engine_with_learned("あいさつ", "挨拶");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));

    // The prediction stays in the list, below candidates for the typed
    // reading, so it is not the initial selection. Step to it.
    let len = engine.state().candidates().unwrap().len();
    for _ in 0..len {
        let selected = engine
            .state()
            .candidates()
            .unwrap()
            .selected()
            .unwrap()
            .clone();
        if selected.text == "挨拶" {
            break;
        }
        engine.process_key(&press_key(Keysym::SPACE));
    }
    let selected = engine
        .state()
        .candidates()
        .unwrap()
        .selected()
        .unwrap()
        .clone();
    assert_eq!(selected.text, "挨拶");
    assert_eq!(selected.reading.as_deref(), Some("あいさつ"));
    assert!(selected.is_deletable());

    engine.process_key(&press_ctrl(Keysym::DELETE));
    assert!(
        engine
            .learning
            .as_ref()
            .unwrap()
            .lookup("あいさつ")
            .is_empty()
    );
}

#[test]
fn aux_shows_delete_hint_only_for_learning_candidate() {
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));

    // Learning candidate selected → aux carries the deletion hint.
    let result = engine.process_key(&press_key(Keysym::SPACE));
    let aux = last_aux_text(&result).expect("conversion must update aux text");
    assert!(
        aux.contains(LEARNING_DELETE_HINT),
        "aux should show the deletion hint for a learning candidate, got {:?}",
        aux,
    );

    // Moving to a non-learning candidate drops the hint.
    let result = engine.process_key(&press_key(Keysym::SPACE));
    let aux = last_aux_text(&result).expect("navigation must update aux text");
    assert!(
        !aux.contains(LEARNING_DELETE_HINT),
        "aux must not show the deletion hint for non-learning candidates, got {:?}",
        aux,
    );
}

#[test]
fn space_key_keeps_learning_in_composing() {
    // Counterpart to tab_key_skips_learning_in_composing: Space stays on the
    // learning-included path so the default UX is unchanged.
    let mut engine = engine_with_learned("あい", "藍");

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));

    let result = engine.process_key(&press_key(Keysym::SPACE));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    let texts: Vec<String> = engine
        .state()
        .candidates()
        .unwrap()
        .candidates()
        .iter()
        .map(|c| c.text.clone())
        .collect();
    assert!(
        texts.contains(&"藍".to_string()),
        "Space must surface learned `藍`, got {:?}",
        texts,
    );
}

// --- Learned homophones: the history against the model's reading of the context ---

/// Engine whose history holds `surfaces` for はいしゃ, most picked first.
fn engine_with_haisha_history(surfaces: &[&str]) -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    let mut cache = LearningCache::new(LearningConfig::default());
    for (rank, surface) in surfaces.iter().enumerate() {
        for _ in rank..surfaces.len() {
            cache.record("はいしゃ", surface);
        }
    }
    engine.learning = Some(cache);
    engine
}

fn haisha_candidates(engine: &mut InputMethodEngine) -> Vec<String> {
    engine
        .build_conversion_candidates("はいしゃ", "はいしゃ", "", 9)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn context_the_model_is_sure_of_reorders_learned_homophones() {
    // The reported case: 歯医者 picked more often than 廃車, then
    // 「バイクと事故ったので」 typed before はいしゃ. The scores are the
    // shipped light model's for that context.
    let mut engine = engine_with_haisha_history(&["歯医者", "廃車"]);
    engine.set_surrounding_context("バイクと事故ったので", "");
    seed_scored_model_cache(
        &mut engine,
        "ハイシャ",
        "バイクと事故ったので",
        &[("廃車", 81.3), ("歯医者", 12.1), ("配車", 5.5)],
    );

    let texts = haisha_candidates(&mut engine);

    assert_eq!(&texts[..4], ["はいしゃ", "廃車", "歯医者", "配車"]);
}

#[test]
fn context_overrules_a_history_that_points_the_other_way() {
    // The README's example with the history against it.
    let mut engine = engine_with_haisha_history(&["廃車", "歯医者"]);
    engine.set_surrounding_context("虫歯の治療のために", "");
    seed_scored_model_cache(
        &mut engine,
        "ハイシャ",
        "虫歯の治療のために",
        &[("歯医者", 97.9), ("廃車", 1.2)],
    );

    let texts = haisha_candidates(&mut engine);

    assert_eq!(&texts[..3], ["はいしゃ", "歯医者", "廃車"]);
}

#[test]
fn an_unsure_model_leaves_the_history_order() {
    // No context: the model leans to 歯医者 by 37% to 29%, which is not
    // enough to overrule what the user picked more often.
    let mut engine = engine_with_haisha_history(&["廃車", "歯医者"]);
    seed_scored_model_cache(
        &mut engine,
        "ハイシャ",
        "",
        &[("歯医者", 37.4), ("廃車", 29.4), ("配車", 16.0)],
    );

    let texts = haisha_candidates(&mut engine);

    assert_eq!(&texts[..4], ["はいしゃ", "廃車", "歯医者", "配車"]);
}

#[test]
fn an_unscored_conversion_leaves_the_history_order() {
    // Greedy decoding returns no probabilities, so there is nothing to
    // weigh the history against.
    let mut engine = engine_with_haisha_history(&["歯医者", "廃車"]);
    seed_model_cache(&mut engine, "ハイシャ", "", &["廃車"]);

    let texts = haisha_candidates(&mut engine);

    assert_eq!(&texts[..3], ["はいしゃ", "歯医者", "廃車"]);
}

#[test]
fn context_lifts_a_learned_candidate_from_below_the_cap() {
    // Four learned homophones and room for three: the one the sentence
    // calls for is chosen before the cap, not cut by it.
    let mut engine = engine_with_haisha_history(&["歯医者", "敗者", "配車", "廃車"]);
    seed_scored_model_cache(
        &mut engine,
        "ハイシャ",
        "",
        &[
            ("廃車", 98.0),
            ("歯医者", 1.0),
            ("敗者", 0.5),
            ("配車", 0.5),
        ],
    );

    let candidates = engine
        .build_conversion_candidates("はいしゃ", "はいしゃ", "", 9)
        .candidates;

    assert_eq!(candidates[1].text, "廃車");
    assert_eq!(candidates[1].source, CandidateSource::Learning);
}

fn learned(texts: &[&str]) -> Vec<Candidate> {
    texts
        .iter()
        .map(|text| Candidate {
            text: text.to_string(),
            reading: Some("はいしゃ".to_string()),
            source: Some(CandidateSource::Learning),
            description: None,
        })
        .collect()
}

fn ranked(history: &[&str], percents: &[(&str, f32)]) -> Vec<String> {
    let scores: Vec<(String, f32)> = percents
        .iter()
        .map(|&(text, percent)| (text.to_string(), (percent / 100.0).ln()))
        .collect();
    rank_learned_by_context(learned(history), &scores, "はいしゃ", |_| None)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn each_history_rank_costs_the_same_odds() {
    // ×4 passes one rank (the bar is ×3) but not two (×9).
    assert_eq!(
        ranked(&["歯医者", "廃車"], &[("廃車", 48.0), ("歯医者", 12.0)]),
        ["廃車", "歯医者"],
    );
    assert_eq!(
        ranked(
            &["歯医者", "敗者", "廃車"],
            &[("廃車", 48.0), ("歯医者", 12.0), ("敗者", 12.0)],
        ),
        ["歯医者", "廃車", "敗者"],
    );
}

#[test]
fn the_learned_kana_keeps_its_place_and_takes_no_rank() {
    // Enter on a composition records the kana, so it sits somewhere in
    // nearly every history. Counted as a rank it would put two ranks
    // between 歯医者 and 廃車 and ×4 would no longer be enough.
    assert_eq!(
        ranked(
            &["歯医者", "はいしゃ", "廃車"],
            &[("廃車", 48.0), ("歯医者", 12.0)],
        ),
        ["廃車", "はいしゃ", "歯医者"],
    );
}

#[test]
fn a_learned_candidate_the_model_did_not_return_scores_as_its_least_likely() {
    // 敗者 is not in the beam: it is taken to be as likely as 配車, the
    // least likely candidate that is. 廃車 is far enough ahead of that to
    // pass both ranks; 歯医者, at ×2.4, stays behind 敗者.
    assert_eq!(
        ranked(
            &["敗者", "歯医者", "廃車"],
            &[("廃車", 81.0), ("歯医者", 12.0), ("配車", 5.0)],
        ),
        ["廃車", "敗者", "歯医者"],
    );
}

// --- Corrections remembered with their context ---

const NIHAITTERUNO: &str = "にはいってるの";

/// Engine with an empty history and a model that, whatever precedes the
/// reading, is sure of 「に入ってるの」 — as the shipped ones are.
fn engine_sure_of_haitteru(contexts: &[&str]) -> InputMethodEngine {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.learning = Some(LearningCache::new(LearningConfig::default()));
    for context in contexts {
        seed_scored_model_cache(
            &mut engine,
            "ニハイッテルノ",
            context,
            &[("に入ってるの", 97.0), ("には行ってるの", 0.2)],
        );
    }
    engine
}

/// Type the reading after `context` and press Space; the selected row.
fn convert_after(engine: &mut InputMethodEngine, context: &str) -> String {
    engine.set_surrounding_context(context, "");
    for ch in "nihaitteruno".chars() {
        engine.process_key(&press(ch));
    }
    engine.process_key(&press_key(Keysym::SPACE));
    selected_text(engine)
}

fn selected_text(engine: &InputMethodEngine) -> String {
    engine
        .state()
        .candidates()
        .and_then(|list| list.selected_text())
        .expect("a selected candidate")
        .to_string()
}

/// Step forward to `surface` and commit it.
fn pick(engine: &mut InputMethodEngine, surface: &str) {
    for _ in 0..20 {
        if selected_text(engine) == surface {
            engine.process_key(&press_key(Keysym::RETURN));
            return;
        }
        engine.process_key(&press_key(Keysym::SPACE));
    }
    panic!("`{surface}` is not in the list");
}

fn remembered(engine: &InputMethodEngine, surface: &str, context: &str) -> bool {
    engine
        .learning
        .as_ref()
        .unwrap()
        .context_match(NIHAITTERUNO, surface, context)
        .is_some()
}

#[test]
fn a_correction_holds_in_its_context_against_a_sure_model() {
    let mut engine = engine_sure_of_haitteru(&["学校", "部屋", "最近は学校", "お風呂"]);

    // The model's answer is wrong here; the user corrects it.
    assert_eq!(convert_after(&mut engine, "学校"), "に入ってるの");
    pick(&mut engine, "には行ってるの");
    // Elsewhere the model was right, and now both are learned — the case
    // where the model used to win everywhere.
    convert_after(&mut engine, "部屋");
    pick(&mut engine, "に入ってるの");

    assert_eq!(convert_after(&mut engine, "学校"), "には行ってるの");
    engine.process_key(&press_key(Keysym::ESCAPE));
    engine.process_key(&press_key(Keysym::ESCAPE));
    assert_eq!(convert_after(&mut engine, "最近は学校"), "には行ってるの");
    engine.process_key(&press_key(Keysym::ESCAPE));
    engine.process_key(&press_key(Keysym::ESCAPE));
    assert_eq!(convert_after(&mut engine, "部屋"), "に入ってるの");
    engine.process_key(&press_key(Keysym::ESCAPE));
    engine.process_key(&press_key(Keysym::ESCAPE));
    // No remembered context speaks here: the model decides again.
    assert_eq!(convert_after(&mut engine, "お風呂"), "に入ってるの");
}

#[test]
fn only_a_correction_stores_its_context() {
    let mut engine = engine_sure_of_haitteru(&["学校", "部屋"]);

    // Taking the first candidate offered: learned, but no text is kept.
    assert_eq!(convert_after(&mut engine, "部屋"), "に入ってるの");
    pick(&mut engine, "に入ってるの");
    assert!(!remembered(&engine, "に入ってるの", "部屋"));

    convert_after(&mut engine, "学校");
    pick(&mut engine, "には行ってるの");
    assert!(remembered(&engine, "には行ってるの", "学校"));
    assert!(!remembered(&engine, "に入ってるの", "学校"));
}

#[test]
fn picking_the_kana_stores_no_context() {
    let mut engine = engine_sure_of_haitteru(&["学校"]);

    convert_after(&mut engine, "学校");
    pick(&mut engine, NIHAITTERUNO);

    assert!(!remembered(&engine, NIHAITTERUNO, "学校"));
}

#[test]
fn a_remembered_context_outranks_the_model_and_the_closest_fit_wins() {
    // 歯医者 was corrected to after 「学校」, 廃車 after 「この学校」; the
    // model is sure of 敗者 throughout.
    let fits = |history: &[&str], context: &str| -> Vec<String> {
        let mut cache = LearningCache::new(LearningConfig::default());
        for surface in history {
            cache.record("はいしゃ", surface);
        }
        cache.remember_context("はいしゃ", "歯医者", "学校");
        cache.remember_context("はいしゃ", "廃車", "この学校");
        let scores = vec![("敗者".to_string(), (0.97f32).ln())];
        rank_learned_by_context(learned(history), &scores, "はいしゃ", |surface| {
            cache.context_match("はいしゃ", surface, context)
        })
        .into_iter()
        .map(|c| c.text)
        .collect()
    };
    let history = ["敗者", "廃車", "歯医者"];

    assert_eq!(fits(&history, "学校"), ["歯医者", "廃車", "敗者"]);
    assert_eq!(fits(&history, "この学校"), ["廃車", "歯医者", "敗者"]);
    assert_eq!(fits(&history, "会社"), ["敗者", "廃車", "歯医者"]);
}
