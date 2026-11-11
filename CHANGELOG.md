# Changelog

## 1.8.0 — 2026-11-11

- **Cache levels are suggested from measurements first.** The page now reads
  [`observed.json`](https://github.com/ngvoicu/cache-ttl-probe/blob/main/observed.json), public measurements of how
  long each route really keeps a prompt cache, and suggests a member's level from a run on the **same route and
  model, made through CPA**. A documented lifetime from models.dev is only the fallback. Before, a Devin member
  serving GPT-6 Astra was suggested OpenAI's 30 minutes; measured through Devin it is 5.
- **A level set above the measured one is flagged**: `set to 30 min; measured: 5 min`, with one-click *Use*.
  The tooltip shows the run: date, how it was measured, and the warm checks per wait time.
- What counts: Codex, Devin, Meta and Antigravity logins, OpenRouter entries and OpenAI API keys. For Meta the run
  must match the cache retention your config asks for; for OpenRouter, runs made before CPA marked requests for
  caching are ignored and runs straight to OpenRouter count. A route measured as unreliable at 5 minutes is
  suggested *no cache*. Nothing is applied by itself.

## 1.7.0 — 2026-11-10

- **Cache-hit counts can survive restarts.** Set `stats-file` in the plugin's config and the counts are read at start
  and saved at most once a minute and when the plugin stops. Without it nothing changes: the counts stay in memory.
  The pool tooltip says where the counts are kept; if the file can't be written, the pool line says "counts not
  saved" with the reason. Keep the file outside CPA's auth directory.

## 1.6.0 — 2026-11-10

- **Search instead of a dropdown.** Creating a pool and *Switch to…* now use a search box instead of a list of
  every model (hundreds with OpenRouter). Type part of a model, an account or a provider (`opus`, `outlook`,
  `devin fable`); results are newest first (catalog release date, then version) and each row names the accounts and
  keys that serve the model. Arrow keys and Enter work. Models already used by another pool are shown greyed out.

## 1.5.1 — 2026-11-09

- **Cache hits say when counting started.** The counts live in memory, so after CPA or the plugin restarts, "last 24 h"
  covered only the time since the restart and looked like a whole day. Windows shorter than their name are now
  labelled by their start: `since 23:12`. Under an hour old, the pool shows one window instead of two identical ones.

## 1.5.0 — 2026-11-09

- **New cache level: 2 hours.** For routes whose cache is measured to last hours, such as GPT-6.1 Sol on a
  ChatGPT/Codex login (warm at 1, 2 and 3 hours in our measurements). Clients are told 7,200 seconds, so pi stops
  warming or compacting a cache that is still alive after an hour.
- **The *longer* level is gone.** It promised more than it said: clients were told 1 hour whatever the route did. Members
  set to *longer* are read as *1 hour* (no change for clients); the review lists them, and the next save stores `1h`.
- **models.dev suggestions:** a documented lifetime of 2 hours or more suggests *2 hours* (was *longer*).

## 1.4.2 — 2026-11-09

- **Stored limits follow the catalogs.** The limits reported to clients were saved once and then stayed, so after a
  catalog change (e.g. CPA's Devin list going from 64k to the real output limits) model lists kept the old numbers
  until someone saved the page. Now, when the page opens and every source of limits has loaded, out-of-date stored
  limits are saved on their own and a note lists what changed. With other changes pending they wait for *Save*.

## 1.4.1 — 2026-11-08

- **Member lines wrap instead of being cut off.** On narrow cards the account line ended in "…", hiding the cache
  level control (and sometimes the limits and hit rate). The model name and the account still shorten with "…", but
  the badges, limits, hits and the cache control move to the next line.
- **No "left out" line under pools.** Sources you left out of a pool are no longer listed; add one back with
  "+ Add account or key…".

## 1.4.0 — 2026-11-08

- **Cache hit rates** per pool and member: the share of prompt tokens read from the cache in the last hour and the last
  day, refreshed every minute, with requests, failures and tokens read and written in the tooltip. The plugin now
  receives CPA's usage records (`usage_plugin`) and counts requests to pools in memory; the page reads them from
  `GET /v0/management/cpa-pools-and-aliases/cache-stats` (management key). Anthropic-style and OpenAI-style token
  counts are both read correctly.

## 1.3.5 — 2026-11-06

- Save no longer lists *cache levels* (or *left-out sources*) as changed when nothing changed. CPA returns the stored
  maps in its own key order and the page compared them in pool order; the comparison now ignores key order.

## 1.3.4 — 2026-11-06

- Cache suggestions for OpenRouter entries follow the model's documented lifetime (e.g. Claude 5 min) instead of
  *no cache*: CPA builds with router-for-me/CLIProxyAPI#7284 mark OpenRouter requests for caching. Other
  OpenAI-compatible entries keep the *no cache* suggestion for request-only caching models.

## 1.3.3 — 2026-11-06

- Limits for members CPA has no data for come from OpenRouter's model list (OpenRouter entries) and models.dev v2,
  the smaller value of each when both have one (GPT-6.1 Sol: 922k input). Shown with their source and counted for the
  pool's limits; *limits unknown* only when neither has the model.
- Cache level suggestions for members without a level, from models.dev, with one-click *Use*. On OpenAI-compatible
  entries, models that only cache on request (Claude) are suggested *no cache*, because CPA sends them no cache markers.
- Both lists are fetched in the background and cached in the browser for a day.

## 1.3.2 — 2026-11-06

- Empty page text explains main and extra pools instead of the old free-name example.

## 1.3.1 — 2026-11-06

- Rename a pool's name prefix any time (field next to the pool title; empty = the model's main pool): main → extra
  keeps members serving the plain id and adds `<name>/` aliases; extra → main drops them, and sources that serve the id
  directly join, as in every main pool.
- Red: *limits unknown* and *cache not set* (member and pool summary). Yellow: mixed cache levels, now saying clients
  plan for the shortest.

## 1.3.0 — 2026-11-05

- **Extra pools for the same model:** a name turns a new pool into `<name>/<model>` (e.g. `subs/claude-opus-5-5`)
  with hand-picked members next to the model's main pool. Any account or key can be in several pools; extra pools don't
  auto-join new sources.
- **Remove any member, including direct ones:** API keys drop the model from their list; accounts keep it only through
  extra-pool aliases (`fork: false`) or get it in `excluded_models`. *+ Add* puts them back.
- **Limits and cache lifetime for clients:** saving stores each pool's smallest context window and output limit, and
  its cache lifetime when every member caches (`plugins.configs.cpa-pools-and-aliases.limits`). The plugin writes them
  into pool entries in model lists (`context_window`, `max_context_window`, `max_tokens`, `prompt_cache_ttl_seconds`;
  Gemini `inputTokenLimit`, `outputTokenLimit`), so they no longer depend on which member CPA registered last.

## 1.2.1 — 2026-11-05

- Limits: members are flagged only when they are more than 10% below the others (Meta's 1.01M next to Devin's 1.05M
  is no longer a warning).

## 1.2.0 — 2026-11-05

- Limits: each pool member shows its context window and output limit (from CPA's model catalog for its provider;
  `max-context-length` in the config overrides the window), each pool shows the smallest values ("limits: 272k
  context · 64k output (smallest; members differ)"), and members with lower limits than the others are flagged with a
  one-click *Leave out*. CPA tells clients one member's limits per pool, so a long conversation can fail, or a reply
  end early, on a member with lower limits. Members without catalog data (OpenAI-compatible) show "limits unknown".
  Nothing is stored and no request changes.

## 1.1.0 — 2026-11-05

- Cache levels: each pool member shows how long it keeps the prompt cache (Claude logins 1 hour, Claude API keys
  5 minutes; other members are set per pool: no cache, 5 min, 30 min, 1 hour or longer), each pool shows a summary
  ("cache: 5 min – 1 hour (mixed)"), and members that don't cache are flagged with a one-click *Leave out*. A member
  marked *no cache* doesn't rejoin automatically. Stored in `plugins.configs.cpa-pools-and-aliases.cache`.

## 1.0.0 — 2026-11-04

First public release.

- Pools page in the CPA console: one model per pool, served by every account and API key that offers it
  (`devin/gpt-6-1-sol` and `gpt-6.1-sol` match: vendor prefix, case and punctuation are ignored).
- Pools are CPA's own model aliases: members that serve the exact id are in automatically (*direct*), others get an
  alias. Disabling the plugin keeps every pool working.
- New accounts and keys join matching pools the next time the page opens; removed members stay out (`skip`).
- *Switch to…* moves a whole pool to another model (e.g. when a new version ships).
- *Clients see: Pools only* hides every other model from model lists (requests by name still work).
- Model lists show pools under their plain model id, never with a provider suffix such as "(Devin)", in OpenAI,
  Claude, Gemini and Codex-style lists.
- Every save shows a review of the exact changes first; deleting a pool removes only the aliases it created.
- API-key groups without a key are ignored.
