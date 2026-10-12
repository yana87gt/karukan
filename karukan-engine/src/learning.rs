//! Learning cache for remembering user-selected conversion results.
//!
//! Records which surface forms the user chose for each reading, and
//! boosts those candidates on subsequent conversions. Persisted as a
//! simple TSV file (`reading\tsurface\tfrequency\tlast_access`, then one
//! `context\tlast_access` pair per context the surface was corrected in).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single learned conversion entry.
#[derive(Debug, Clone)]
pub struct LearningEntry {
    /// Surface form (e.g. "今日")
    pub surface: String,
    /// Number of times this surface was selected
    pub frequency: u32,
    /// Last selection time as Unix timestamp (seconds)
    pub last_access: u64,
    /// Left contexts this surface was picked in over the candidate offered
    /// first; see [`LearningCache::remember_context`].
    pub contexts: Vec<LearnedContext>,
}

/// The text left of the caret when a surface was picked over the candidate
/// offered first.
#[derive(Debug, Clone)]
pub struct LearnedContext {
    pub text: String,
    /// Last time the surface was picked with this text before it
    pub last_access: u64,
}

/// How well a remembered context fits the text now left of the caret.
/// Ordered by fit: more shared chars first, then the remembered context
/// that had less besides them — 「学校」 fits 「学校」 better than
/// 「この学校」 does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMatch {
    /// Chars the remembered context and the current one end with alike
    pub shared: usize,
    /// Chars of the remembered context before the shared ones
    pub missing: usize,
}

impl Ord for ContextMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.shared
            .cmp(&other.shared)
            .then_with(|| other.missing.cmp(&self.missing))
    }
}

impl PartialOrd for ContextMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Contexts kept per surface; the least recently used one makes room.
const MAX_CONTEXTS_PER_SURFACE: usize = 8;

/// The shortest context worth remembering, and the least a match must
/// share: one character (a particle, a comma) precedes too many things to
/// say which one is meant.
const MIN_CONTEXT_CHARS: usize = 2;

/// A context as it is stored and compared: on one line of the TSV, so tabs
/// and other control characters become spaces. `None` when what is left is
/// too short to mean anything.
fn normalize_context(context: &str) -> Option<String> {
    let text: String = context
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    (text.trim().chars().count() >= MIN_CONTEXT_CHARS).then_some(text)
}

/// How `stored` fits `context`, or `None` when it does not: they must end
/// alike for at least half of what was stored. The conversion follows the
/// text right before it, so the end is what has to agree, and half keeps a
/// shared 「ので」 from standing in for a whole remembered clause.
fn match_context(stored: &str, context: &str) -> Option<ContextMatch> {
    let shared = stored
        .chars()
        .rev()
        .zip(context.chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let missing = stored.chars().count() - shared;
    (shared >= MIN_CONTEXT_CHARS && shared >= missing).then_some(ContextMatch { shared, missing })
}

/// Size limits for a [`LearningCache`].
///
/// Passed whole at construction ([`LearningCache::new`] /
/// [`LearningCache::load`]) so a caller can't apply one limit and forget the
/// other — there is no post-construction setter to miss.
#[derive(Debug, Clone, Copy)]
pub struct LearningConfig {
    /// Maximum number of total entries across all readings; lowest-score
    /// entries are evicted on save when over this limit.
    pub max_entries: usize,
    /// Maximum surface length (Unicode chars) that [`LearningCache::record`]
    /// accepts. Keeps whole-sentence live-conversion commits — one-off text
    /// that never matches again — out of the cache.
    pub max_surface_chars: usize,
}

impl LearningConfig {
    /// Default for [`max_entries`](Self::max_entries).
    pub const DEFAULT_MAX_ENTRIES: usize = 10_000;
    /// Default for [`max_surface_chars`](Self::max_surface_chars).
    pub const DEFAULT_MAX_SURFACE_CHARS: usize = 50;
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            max_entries: Self::DEFAULT_MAX_ENTRIES,
            max_surface_chars: Self::DEFAULT_MAX_SURFACE_CHARS,
        }
    }
}

/// In-memory cache of user learning data.
///
/// Keyed by reading (hiragana). Each reading maps to a list of surface
/// entries with frequency and recency metadata.
#[derive(Debug)]
pub struct LearningCache {
    entries: HashMap<String, Vec<LearningEntry>>,
    max_entries: usize,
    max_surface_chars: usize,
    dirty: bool,
}

impl LearningCache {
    /// Create an empty cache with the given limits.
    pub fn new(config: LearningConfig) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: config.max_entries,
            max_surface_chars: config.max_surface_chars,
            dirty: false,
        }
    }

    /// Record a user selection. Increments frequency and updates last_access.
    ///
    /// Surfaces longer than `max_surface_chars` are skipped; see
    /// [`LearningConfig::max_surface_chars`] for why.
    pub fn record(&mut self, reading: &str, surface: &str) {
        if surface.chars().count() > self.max_surface_chars {
            return;
        }
        let now = now_unix();
        let entries = self.entries.entry(reading.to_string()).or_default();

        if let Some(entry) = entries.iter_mut().find(|e| e.surface == surface) {
            entry.frequency += 1;
            entry.last_access = now;
        } else {
            entries.push(LearningEntry {
                surface: surface.to_string(),
                frequency: 1,
                last_access: now,
                contexts: Vec::new(),
            });
        }
        self.dirty = true;
    }

    /// Remember that `surface` was picked for `reading` with `context` left
    /// of the caret, over the candidate offered first. Called for
    /// corrections only: where the first candidate was right nothing needs
    /// remembering, and the text before a conversion is not written to disk
    /// without a reason.
    ///
    /// The same context on another surface of the reading is dropped — the
    /// latest correction is what the context means now. No-op for a surface that
    /// is not learned (see [`Self::record`]) or a context too short to tell
    /// anything apart.
    pub fn remember_context(&mut self, reading: &str, surface: &str, context: &str) {
        let Some(context) = normalize_context(context) else {
            return;
        };
        let Some(entries) = self.entries.get_mut(reading) else {
            return;
        };
        if !entries.iter().any(|e| e.surface == surface) {
            return;
        }
        let now = now_unix();
        for entry in entries.iter_mut() {
            if entry.surface != surface {
                entry.contexts.retain(|c| c.text != context);
                continue;
            }
            match entry.contexts.iter_mut().find(|c| c.text == context) {
                Some(known) => known.last_access = now,
                None => entry.contexts.push(LearnedContext {
                    text: context.clone(),
                    last_access: now,
                }),
            }
            if entry.contexts.len() > MAX_CONTEXTS_PER_SURFACE
                && let Some(oldest) =
                    (0..entry.contexts.len()).min_by_key(|&i| entry.contexts[i].last_access)
            {
                entry.contexts.remove(oldest);
            }
        }
        self.dirty = true;
    }

    /// `surface` was offered first with `context` left of the caret and
    /// taken: keep the remembered context that put it there in use, so a
    /// correction that keeps working is not the first one forgotten.
    /// Nothing new is stored.
    pub fn confirm_context(&mut self, reading: &str, surface: &str, context: &str) {
        let Some(context) = normalize_context(context) else {
            return;
        };
        let Some(entry) = self
            .entries
            .get_mut(reading)
            .and_then(|entries| entries.iter_mut().find(|e| e.surface == surface))
        else {
            return;
        };
        let best = entry
            .contexts
            .iter_mut()
            .filter_map(|c| Some((match_context(&c.text, &context)?, c)))
            .max_by_key(|&(fit, _)| fit);
        if let Some((_, known)) = best {
            known.last_access = now_unix();
            self.dirty = true;
        }
    }

    /// The best fit between `context` and the contexts `surface` was
    /// corrected to under `reading`, if any fits.
    pub fn context_match(
        &self,
        reading: &str,
        surface: &str,
        context: &str,
    ) -> Option<ContextMatch> {
        let context = normalize_context(context)?;
        self.entries
            .get(reading)?
            .iter()
            .find(|e| e.surface == surface)?
            .contexts
            .iter()
            .filter_map(|c| match_context(&c.text, &context))
            .max()
    }

    /// Remove every learned entry that would resurface `surface` for input
    /// `reading`: the exact-reading entry plus every longer reading with
    /// `reading` as a prefix (the [`prefix_lookup`](Self::prefix_lookup)
    /// fan-out) — an exact-only delete would leave a twin that pops back on
    /// the next conversion. Returns whether anything was removed; persisted
    /// at the next `save`.
    pub fn remove_suggestion(&mut self, reading: &str, surface: &str) -> bool {
        let mut removed = false;
        self.entries.retain(|r, entries| {
            if !r.starts_with(reading) {
                return true;
            }
            let before = entries.len();
            entries.retain(|e| e.surface != surface);
            removed |= entries.len() != before;
            !entries.is_empty()
        });
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Exact-match lookup: returns `(surface, score)` pairs sorted by score descending.
    pub fn lookup(&self, reading: &str) -> Vec<(String, f64)> {
        let now = now_unix();
        let Some(entries) = self.entries.get(reading) else {
            return Vec::new();
        };
        let mut scored: Vec<(String, f64)> = entries
            .iter()
            .map(|e| (e.surface.clone(), score(e, now)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored
    }

    /// Prefix-match lookup: returns `(reading, surface, score)` triples
    /// for all readings that start with `prefix`, sorted by score descending.
    /// Equal scores are common (the score steps by whole days and by
    /// frequency), so they fall back to the reading: left to the map's
    /// iteration order, which rows make a capped list would change from one
    /// launch to the next.
    pub fn prefix_lookup(&self, prefix: &str) -> Vec<(String, String, f64)> {
        let now = now_unix();
        let mut results: Vec<(String, String, f64)> = Vec::new();
        for (reading, entries) in &self.entries {
            if reading.starts_with(prefix) {
                for entry in entries {
                    results.push((reading.clone(), entry.surface.clone(), score(entry, now)));
                }
            }
        }
        results.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        results
    }

    /// Load a learning cache from a TSV file.
    ///
    /// Format: `reading\tsurface\tfrequency\tlast_access`, optionally
    /// followed by `context\tlast_access` pairs (absent in a v1 file).
    /// Lines starting with `#` are comments.
    pub fn load(path: &Path, config: LearningConfig) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut cache = Self::new(config);

        for line in reader.lines() {
            let line = line?;
            // Not the whole line: a context may begin or end with a space.
            let line = line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                continue;
            }
            // The line used to be trimmed whole; keep reading files that
            // relied on it.
            let reading = parts[0].trim_start();
            let surface = parts[1];
            let frequency: u32 = match parts[2].trim().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let last_access: u64 = match parts[3].trim().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            cache
                .entries
                .entry(reading.to_string())
                .or_default()
                .push(LearningEntry {
                    surface: surface.to_string(),
                    frequency,
                    last_access,
                    contexts: parts[4..]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .filter_map(|[text, last_access]| {
                            Some(LearnedContext {
                                text: normalize_context(text)?,
                                last_access: last_access.trim().parse().ok()?,
                            })
                        })
                        .collect(),
                });
        }

        // Not dirty — just loaded from disk
        cache.dirty = false;
        Ok(cache)
    }

    /// Save the cache to a TSV file, evicting low-score entries if over capacity.
    ///
    /// The file is written beside `path` and renamed over it, so a save
    /// that fails part-way (a full disk, a crash) leaves the previous file
    /// whole. Writing in place truncates first, and a truncated file loads
    /// as a shorter history with no sign anything was lost.
    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        self.evict();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let tmp = temp_path(path);
        let written = self
            .write_tsv(&tmp)
            .and_then(|()| Ok(std::fs::rename(&tmp, path)?));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        self.dirty = false;
        Ok(())
    }

    /// Write every entry to `path` and flush it to disk.
    fn write_tsv(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        writeln!(writer, "# karukan learning cache v2")?;

        // Sort readings for deterministic output
        let mut readings: Vec<&String> = self.entries.keys().collect();
        readings.sort();

        for reading in readings {
            if let Some(entries) = self.entries.get(reading) {
                for entry in entries {
                    write!(
                        writer,
                        "{}\t{}\t{}\t{}",
                        reading, entry.surface, entry.frequency, entry.last_access
                    )?;
                    for context in &entry.contexts {
                        write!(writer, "\t{}\t{}", context.text, context.last_access)?;
                    }
                    writeln!(writer)?;
                }
            }
        }

        writer.flush()?;
        // On disk before the rename makes it the cache: a crash right after
        // must not leave the new name pointing at unwritten data.
        writer.get_ref().sync_all()?;
        Ok(())
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Total number of (reading, surface) pairs across all readings.
    pub fn entry_count(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }

    /// Evict lowest-score entries until total count is within `max_entries`.
    fn evict(&mut self) {
        let total = self.entry_count();
        if total <= self.max_entries {
            return;
        }

        let now = now_unix();
        // Collect all entries with their (reading, index, score)
        let mut all: Vec<(String, usize, f64)> = Vec::with_capacity(total);
        for (reading, entries) in &self.entries {
            for (i, entry) in entries.iter().enumerate() {
                all.push((reading.clone(), i, score(entry, now)));
            }
        }
        // Sort by score ascending (lowest first = eviction candidates),
        // equal scores by reading so the choice never depends on the map's
        // iteration order
        all.sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

        let to_remove = total - self.max_entries;
        // Collect indices to remove, grouped by reading
        let mut remove_set: HashMap<String, Vec<usize>> = HashMap::new();
        for &(ref reading, idx, _) in all.iter().take(to_remove) {
            remove_set.entry(reading.clone()).or_default().push(idx);
        }

        // Remove entries in reverse index order to preserve indices
        for (reading, indices) in &mut remove_set {
            indices.sort_unstable();
            indices.reverse();
            if let Some(entries) = self.entries.get_mut(reading) {
                for &idx in indices.iter() {
                    if idx < entries.len() {
                        entries.remove(idx);
                    }
                }
                if entries.is_empty() {
                    self.entries.remove(reading);
                }
            }
        }
    }
}

/// Where [`LearningCache::save`] writes before renaming over `path`: the
/// same directory, so the rename never crosses filesystems.
fn temp_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Compute a candidate score: recency-weighted with frequency bonus.
///
/// Inspired by mozc's UserHistoryPredictor: recent selections rank higher,
/// with a logarithmic frequency term to reward repeated use.
fn score(entry: &LearningEntry, now: u64) -> f64 {
    let age_days = if now > entry.last_access {
        (now - entry.last_access) / 86400
    } else {
        0
    };
    let recency = 1.0 / (1.0 + age_days as f64);
    let freq = (entry.frequency as f64).ln_1p();
    recency * 10.0 + freq
}

/// Current time as Unix timestamp in seconds.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config/cache with a custom entry limit and the default surface cap.
    fn config_with(max_entries: usize) -> LearningConfig {
        LearningConfig {
            max_entries,
            ..LearningConfig::default()
        }
    }

    fn cache_with(max_entries: usize) -> LearningCache {
        LearningCache::new(config_with(max_entries))
    }
    use tempfile::NamedTempFile;

    #[test]
    fn test_record_and_lookup() {
        let mut cache = cache_with(100);

        cache.record("きょう", "今日");
        cache.record("きょう", "京");
        cache.record("きょう", "今日"); // frequency bump

        let results = cache.lookup("きょう");
        assert_eq!(results.len(), 2);
        // "今日" should have higher score (frequency 2 vs 1)
        assert_eq!(results[0].0, "今日");
        assert_eq!(results[1].0, "京");
    }

    #[test]
    fn test_lookup_empty() {
        let cache = cache_with(100);
        let results = cache.lookup("きょう");
        assert!(results.is_empty());
    }

    #[test]
    fn test_prefix_lookup() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.record("きょうと", "京都");
        cache.record("あした", "明日");

        let results = cache.prefix_lookup("きょう");
        assert_eq!(results.len(), 2);
        // Both "きょう" and "きょうと" should match
        let readings: Vec<&str> = results.iter().map(|(r, _, _)| r.as_str()).collect();
        assert!(readings.contains(&"きょう"));
        assert!(readings.contains(&"きょうと"));
    }

    #[test]
    fn test_prefix_lookup_orders_equal_scores_by_reading() {
        // Recorded together, once each: every score is the same. Each cache
        // is its own HashMap with its own iteration order.
        let readings = ["よろず", "よろしく", "よろこび", "よろしくたのむ"];
        for _ in 0..20 {
            let mut cache = cache_with(100);
            for reading in readings {
                cache.record(reading, reading);
            }
            let order: Vec<String> = cache
                .prefix_lookup("よろ")
                .into_iter()
                .map(|(reading, _, _)| reading)
                .collect();
            assert_eq!(order, ["よろこび", "よろしく", "よろしくたのむ", "よろず"]);
        }
    }

    #[test]
    fn test_prefix_lookup_keeps_a_readings_surfaces_in_recorded_order() {
        let mut cache = cache_with(100);
        cache.record("きょう", "京");
        cache.record("きょう", "今日");
        let surfaces: Vec<String> = cache
            .prefix_lookup("きょ")
            .into_iter()
            .map(|(_, surface, _)| surface)
            .collect();
        assert_eq!(surfaces, ["京", "今日"], "as `lookup` orders them");
    }

    #[test]
    fn test_prefix_lookup_no_match() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        let results = cache.prefix_lookup("あ");
        assert!(results.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.record("きょう", "今日");
        cache.record("きょう", "京");
        cache.record("あした", "明日");

        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        cache.save(&path).unwrap();
        assert!(!cache.is_dirty());

        let loaded = LearningCache::load(&path, config_with(100)).unwrap();
        assert!(!loaded.is_dirty());
        assert_eq!(loaded.entry_count(), 3);

        let results = loaded.lookup("きょう");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "今日"); // frequency 2
    }

    #[test]
    fn test_save_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learning.tsv");
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.save(&path).unwrap();

        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["learning.tsv"]);
    }

    #[test]
    fn test_failed_save_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learning.tsv");
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();

        // A directory where the temp file goes: the write cannot start.
        std::fs::create_dir(temp_path(&path)).unwrap();
        cache.record("あした", "明日");
        assert!(cache.save(&path).is_err());

        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        assert!(cache.is_dirty(), "still unsaved, so the next save retries");
    }

    #[test]
    fn test_dirty_flag() {
        let mut cache = cache_with(100);
        assert!(!cache.is_dirty());

        cache.record("きょう", "今日");
        assert!(cache.is_dirty());

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();
        assert!(!cache.is_dirty());
    }

    #[test]
    fn test_eviction() {
        let mut cache = cache_with(3);

        // Add 5 entries
        cache.record("a", "A");
        cache.record("b", "B");
        cache.record("c", "C");
        cache.record("d", "D");
        cache.record("e", "E");

        // Boost some to give them higher scores
        cache.record("a", "A");
        cache.record("a", "A");
        cache.record("c", "C");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        // After eviction, should be at most 3 entries
        assert!(cache.entry_count() <= 3);
    }

    #[test]
    fn test_eviction_of_equal_scores_is_the_same_every_time() {
        for _ in 0..20 {
            let mut cache = cache_with(2);
            for reading in ["d", "b", "a", "c"] {
                cache.record(reading, reading);
            }
            let file = NamedTempFile::new().unwrap();
            cache.save(file.path()).unwrap();

            let mut kept: Vec<String> = cache
                .prefix_lookup("")
                .into_iter()
                .map(|(reading, _, _)| reading)
                .collect();
            kept.sort();
            assert_eq!(kept, ["c", "d"]);
        }
    }

    #[test]
    fn test_score_recency() {
        let now = now_unix();
        let recent = LearningEntry {
            surface: "A".to_string(),
            frequency: 1,
            last_access: now,
            contexts: Vec::new(),
        };
        let old = LearningEntry {
            surface: "B".to_string(),
            frequency: 1,
            last_access: now.saturating_sub(30 * 86400), // 30 days ago
            contexts: Vec::new(),
        };
        assert!(score(&recent, now) > score(&old, now));
    }

    #[test]
    fn test_score_frequency() {
        let now = now_unix();
        let high_freq = LearningEntry {
            surface: "A".to_string(),
            frequency: 100,
            last_access: now,
            contexts: Vec::new(),
        };
        let low_freq = LearningEntry {
            surface: "B".to_string(),
            frequency: 1,
            last_access: now,
            contexts: Vec::new(),
        };
        assert!(score(&high_freq, now) > score(&low_freq, now));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = LearningCache::load(Path::new("/nonexistent/path"), config_with(100));
        assert!(result.is_err());
    }

    #[test]
    fn test_tsv_format() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.starts_with("# karukan learning cache v2"));
        assert!(content.contains("きょう\t今日\t1\t"));
    }

    #[test]
    fn test_tsv_comments_and_blanks_ignored() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "# comment\n\nきょう\t今日\t5\t1700000000\n# another comment\n",
        )
        .unwrap();

        let cache = LearningCache::load(file.path(), config_with(100)).unwrap();
        assert_eq!(cache.entry_count(), 1);
        let results = cache.lookup("きょう");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "今日");
    }

    #[test]
    fn test_remove_suggestion_last_surface_drops_reading() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");

        assert!(cache.remove_suggestion("きょう", "今日"));
        assert_eq!(cache.entry_count(), 0);
        assert!(cache.lookup("きょう").is_empty());
        assert!(cache.prefix_lookup("き").is_empty());
    }

    #[test]
    fn test_remove_suggestion_clears_exact_and_prefix_twins() {
        let mut cache = cache_with(100);
        cache.record("あい", "藍");
        cache.record("あいさ", "藍"); // same surface, longer reading (a twin)
        cache.record("あい", "愛"); // different surface under the same reading
        cache.record("うみ", "藍"); // same surface, unrelated reading

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();
        assert!(!cache.is_dirty());

        assert!(cache.remove_suggestion("あい", "藍"));
        assert!(cache.is_dirty(), "removal must mark the cache dirty");

        // Both the exact entry and the prefix twin are gone...
        assert!(cache.lookup("あい").iter().all(|(s, _)| s != "藍"));
        assert!(cache.lookup("あいさ").is_empty());
        // ...but a different surface under the same reading survives...
        assert!(cache.lookup("あい").iter().any(|(s, _)| s == "愛"));
        // ...and the same surface under an unrelated reading is untouched.
        assert!(cache.lookup("うみ").iter().any(|(s, _)| s == "藍"));
    }

    #[test]
    fn test_remove_suggestion_nonexistent_is_noop() {
        let mut cache = cache_with(100);
        cache.record("あい", "藍");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        assert!(!cache.remove_suggestion("あい", "愛"));
        assert!(!cache.remove_suggestion("かき", "柿"));
        assert!(!cache.is_dirty(), "no-op removal must not mark dirty");
    }

    #[test]
    fn test_record_skips_long_surface() {
        let mut cache = LearningCache::new(LearningConfig {
            max_entries: 100,
            max_surface_chars: 5,
        });

        cache.record("あ", &"漢".repeat(6));
        assert_eq!(cache.entry_count(), 0);
        assert!(!cache.is_dirty());

        // Boundary: exactly max_surface_chars is accepted.
        cache.record("あ", &"漢".repeat(5));
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_record_ignores_reading_length() {
        let mut cache = LearningCache::new(LearningConfig {
            max_entries: 100,
            max_surface_chars: 5,
        });

        // Only the surface is capped; a long reading with a short surface
        // is fine (e.g. a long kana reading converting to a short word).
        cache.record(&"あ".repeat(30), "短い");
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_default_max_surface_chars() {
        let mut cache = cache_with(100);

        cache.record(
            "よみ",
            &"あ".repeat(LearningConfig::DEFAULT_MAX_SURFACE_CHARS + 1),
        );
        assert_eq!(cache.entry_count(), 0);

        cache.record(
            "よみ",
            &"あ".repeat(LearningConfig::DEFAULT_MAX_SURFACE_CHARS),
        );
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_tsv_malformed_lines_skipped() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "きょう\t今日\t5\t1700000000\nmalformed_line\nきょう\t京\tbad\t1700000000\n",
        )
        .unwrap();

        let cache = LearningCache::load(file.path(), config_with(100)).unwrap();
        // Only the first valid line should be loaded
        assert_eq!(cache.entry_count(), 1);
    }

    // --- Remembered contexts ---

    fn fit(shared: usize, missing: usize) -> Option<ContextMatch> {
        Some(ContextMatch { shared, missing })
    }

    #[test]
    fn test_context_matches_where_it_recurs() {
        let mut cache = cache_with(100);
        cache.record("にはいってるの", "には行ってるの");
        cache.remember_context("にはいってるの", "には行ってるの", "学校");

        let at = |context| cache.context_match("にはいってるの", "には行ってるの", context);
        assert_eq!(at("学校"), fit(2, 0));
        // What precedes the remembered text does not matter.
        assert_eq!(at("最近は学校"), fit(2, 0));
        assert_eq!(at("お風呂"), None);
        assert_eq!(at(""), None);
        // Another surface of the reading has no context of its own.
        assert_eq!(
            cache.context_match("にはいってるの", "に入ってるの", "学校"),
            None,
        );
    }

    #[test]
    fn test_context_needs_half_of_what_was_remembered() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        cache.remember_context("はいしゃ", "廃車", "バイクと事故ったので");

        let at = |context| cache.context_match("はいしゃ", "廃車", context);
        assert_eq!(at("車と事故ったので"), fit(7, 3));
        assert_eq!(at("事故ったので"), fit(6, 4));
        // A shared 「ので」 is not the remembered clause.
        assert_eq!(at("虫歯が痛いので"), None);
    }

    #[test]
    fn test_context_fit_prefers_the_closer_memory() {
        // Both end in 「学校」; the one that was nothing but 「学校」 is the
        // better fit for it, the longer one for 「この学校」.
        let exact = ContextMatch {
            shared: 2,
            missing: 0,
        };
        let half = ContextMatch {
            shared: 2,
            missing: 2,
        };
        let longer = ContextMatch {
            shared: 4,
            missing: 0,
        };
        assert!(exact > half);
        assert!(longer > exact);
    }

    #[test]
    fn test_context_too_short_is_not_remembered() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        cache.remember_context("はいしゃ", "廃車", "は");
        cache.remember_context("はいしゃ", "廃車", "   ");

        assert!(cache.entries["はいしゃ"][0].contexts.is_empty());
    }

    #[test]
    fn test_context_is_not_remembered_for_an_unlearned_surface() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        cache.remember_context("はいしゃ", "歯医者", "虫歯の治療のために");
        cache.remember_context("よみ", "読み", "虫歯の治療のために");

        assert_eq!(cache.entry_count(), 1);
        assert!(cache.entries["はいしゃ"][0].contexts.is_empty());
    }

    #[test]
    fn test_latest_correction_takes_the_context_over() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        cache.record("はいしゃ", "歯医者");
        cache.remember_context("はいしゃ", "廃車", "事故ったので");
        cache.remember_context("はいしゃ", "歯医者", "事故ったので");

        assert_eq!(
            cache.context_match("はいしゃ", "廃車", "事故ったので"),
            None
        );
        assert_eq!(
            cache.context_match("はいしゃ", "歯医者", "事故ったので"),
            fit(6, 0),
        );
    }

    #[test]
    fn test_context_cap_drops_the_least_recently_used() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        for i in 0..=MAX_CONTEXTS_PER_SURFACE {
            cache.remember_context("はいしゃ", "廃車", &format!("文脈{i}"));
            // Age each one so the first is the oldest.
            for context in &mut cache.entries.get_mut("はいしゃ").unwrap()[0].contexts {
                context.last_access -= 10;
            }
        }
        // Using the oldest survivor keeps it; the next oldest goes instead.
        cache.confirm_context("はいしゃ", "廃車", "文脈1");
        cache.remember_context("はいしゃ", "廃車", "新しい文脈");

        let texts: Vec<&str> = cache.entries["はいしゃ"][0]
            .contexts
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(texts.len(), MAX_CONTEXTS_PER_SURFACE);
        assert!(!texts.contains(&"文脈0"));
        assert!(texts.contains(&"文脈1"));
        assert!(!texts.contains(&"文脈2"));
        assert!(texts.contains(&"新しい文脈"));
    }

    #[test]
    fn test_contexts_survive_save_and_load() {
        let mut cache = cache_with(100);
        cache.record("はいしゃ", "廃車");
        cache.record("はいしゃ", "歯医者");
        // A tab would split the line; a trailing space is part of the text.
        cache.remember_context("はいしゃ", "廃車", "事故\tった ");
        cache.remember_context("はいしゃ", "廃車", "バイクと事故ったので");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();
        let loaded = LearningCache::load(file.path(), config_with(100)).unwrap();

        assert_eq!(loaded.entry_count(), 2);
        assert_eq!(
            loaded.context_match("はいしゃ", "廃車", "事故 った "),
            fit(6, 0),
        );
        assert_eq!(
            loaded.context_match("はいしゃ", "廃車", "バイクと事故ったので"),
            fit(10, 0),
        );
        assert_eq!(
            loaded.context_match("はいしゃ", "歯医者", "バイクと事故ったので"),
            None,
        );
    }

    #[test]
    fn test_v1_file_loads_without_contexts() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "# karukan learning cache v1\nはいしゃ\t廃車\t2\t1700000000\n",
        )
        .unwrap();

        let cache = LearningCache::load(file.path(), config_with(100)).unwrap();

        assert_eq!(cache.entry_count(), 1);
        assert!(cache.entries["はいしゃ"][0].contexts.is_empty());
    }
}
