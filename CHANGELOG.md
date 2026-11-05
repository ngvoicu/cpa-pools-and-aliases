# Changelog

## 1.1.0 — unreleased

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
