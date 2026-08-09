# Completion fixtures — canonical spec for the unified completion engine

Array of cases per file, describing what the **unified** engine must return (post-unification
canon, not today's behavior). Validated by `tests/completion_fixtures.rs`; unknown keys rejected.
`decisions` tags (`D1`–`D20`) index the owner's engine-unification decision table, which is
maintained outside this repository.

- `name` snake_case unique suite-wide · `notes` 1-liner (starts with `OPEN:` when `expect` is
  `null`) · `decisions` `^D([1-9]|1[0-9]|20)$` or `[]`.
- `doc` + `pos` `{line, character}`, 0-based, **UTF-16 units** (plan §2); `trigger` = the D1
  trigger char or `null` (invoked/typing).
- `settings` omitted ⇒ suite defaults below, else all four keys (`arpeggio_pattern`:
  up|down|updown|downup|alberti, `chord_stack_mode`: stack_up|plain).
- `data`: `null` or a resolve payload — `fm_patches` (`rel_path`/`name`/`mml`/`has_macros`), `pcm_files`, `pcm_paths`, `cursor_tick` (`tick: null` = host failure).
- `expect`: `null` (OPEN) or `{plan, is_incomplete, items}` / `{plan, is_incomplete, count,
  items_spot}`; `plan` = `"done"` or `{"needs": …}`, must agree with `data`'s kind.
- Item fields: `label` (required), `kind`, `detail`, `documentation_contains` (substring),
  `insert`, `insert_format` (plain|snippet), `filter_text`, `sort_text`, `preselect`, `range`
  (`{start_character,end_character}`; cursor line unless `start_line`/`end_line`),
  `additional_edits`, `command` (`trigger_suggest`), and `assert_absent`. `assert_absent` is an
  optional list containing any of `command`, `documentation`, `label_description`, `detail`,
  `sort_text`, or `filter_text`; each named optional field must be absent/`None` on the produced
  item. **Omitted = don't assert** — hence `count` + `items_spot` (`{index, …item fields}`) for
  long lists.
- Ranges are always explicit (D17); where the old code used Monaco's word range the canon is the
  run of `[A-Za-z0-9_]` before the cursor. Meta-value items instead replace the non-whitespace
  value token before the cursor, so punctuation such as `/` and `-` is included. Table-driven
  providers (meta, meta values, instrument types, rate/offset, commands, platform commands) share
  one rendering: table `detail`/`doc`, `filter_text` = table key, `sort_text` = 3-digit index.

**Suite defaults** = megamml defaults (`megamml/src/settings/completion-prefs.ts`):
`arpeggio_enabled: false` (L29-31 opt-in; L27 "When false (default)"), `arpeggio_pattern: "up"`
(L41-46), `chord_stack_mode: "stack_up"` (L8-10; TS calls the other mode `letter`),
`fm_picker_hierarchy: false` (no megamml setting — native-only flag, D12).

The loader validates and executes all 76 fixtures. Each case first runs `completion_plan` and
asserts `expect.plan`; when the plan needs data, the harness converts the fixture's `data` to a
`DataPayload`, calls `completion_resolve`, and compares that resolved list against `items` or
`count` + `items_spot` (including `additional_edits` and `assert_absent`). The pending list is
required to remain empty.
