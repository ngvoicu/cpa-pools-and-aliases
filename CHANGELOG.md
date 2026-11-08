# Changelog

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
