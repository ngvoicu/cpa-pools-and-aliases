# Pools and Aliases for CLIProxyAPI

<img src="assets/logo.svg" width="64" align="right" alt="">

A [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) (CPA) plugin that adds a **Pools** page to the
management console.

A pool is **one model served by several accounts or keys**. Every source names the same model differently:
Devin has `devin/claude-opus-5-5`, Claude Code has `claude-opus-5-5`, an API key has whatever you configured.
Pick a model and every source that offers it joins. Clients ask for `claude-opus-5-5` and CPA spreads
requests across all members, while your Routing tiers (Preferred → Fallback 1 → …) still decide the order.

```
┌ claude-opus-5-5 ──────────────────── ✕ ┐   Models (drag onto a pool)
│ 5 sources                 Switch to… ▾ │   ▾ Claude  gabriel@…      claude-opus-5-5 …
│ claude-opus-5-5  DIRECT  Claude · …    │   ▾ Devin   gabriel@…      devin/claude-opus-5-5 …
│ claude-opus-5-5  DIRECT  API key c-1   │   ▾ Claude API key claude-1 …
│ devin/claude-opus-5-5    Devin · …   ✕ │
└────────────────────────────────────────┘
```

## How it works

- **One model per pool.** A pool's CPA name is the model's own id (`claude-opus-5-5`), so clients price it,
  size its context and pick reasoning levels as that model, and failover never silently changes models.
  Sources match across providers by ignoring the `vendor/` prefix, case and punctuation
  (`devin/gpt-6-1-sol` = `gpt-6.1-sol`). Dropping a different model on a pool is refused.
- **Pools are CPA's own model aliases.** Members that already serve the exact id are in the pool
  automatically (marked *direct*); others get an alias `<their model> → <pool id>`. The plugin doesn't
  route or schedule anything. Disable it and your pools keep working.
- **No provider names.** In every model list CPA serves (OpenAI, Claude, Gemini and Codex-style clients such
  as pi) a pool shows as its plain model id, never as `Claude Opus 5.5 (Devin)`. Regular models keep their
  own names. Pools have no separate names: the model id is what clients send, so that's what they see.
- **New sources join automatically.** Every account or key that offers a pool's model is a member. Add a
  new account or key (for example OpenRouter, whose `anthropic/claude-opus-5.5` matches `claude-opus-5-5`)
  and the next time you open the page it is added to the matching pools, ready for review and save.
  Remove a member and it stays out (`skip` in the plugin config) until you add it back. API-key groups
  without a key are ignored.
- **Switch to…** moves every member to another model in one save, e.g. when Opus 6 ships. Members that
  don't offer the new model leave the pool.
- **Clients see: Pools and models / Pools only.** With *Pools only*, model lists show just your pools; all
  other models disappear from client pickers. It only hides them: a client that asks for a hidden model by
  name still gets it. Stored as `pools-only: true` in the plugin config.
- **Only its own pools.** The pool list is kept in `plugins.configs.cpa-pools-and-aliases.pools` as a list
  of model ids. Aliases for other names are never shown or changed.
- Every save shows a review of the exact changes first. Deleting a pool removes only the aliases it created;
  sources that serve the model directly keep serving it.
- Pools from v0.4 and earlier had their own names (`zeus`). The page renames them to their model id and
  removes the old names; review and save once. Clients that asked for the old name must use the model id.

Where a pool member is saved:

| Source | Saved to | API used |
|---|---|---|
| Account (OAuth login) | `model_aliases` in that credential's JSON file | `PATCH /v8/management/credentials/fields` |
| API key | `api-keys.<provider>[group](.keys[i]).models[]` | `PUT /v8/management/config/api-keys/<provider>` |
| Pool list | `plugins.configs.cpa-pools-and-aliases.pools` | `PUT /v8/management/config/plugins/configs/cpa-pools-and-aliases/pools` |

> **API keys without a model list** serve every default model of their provider. Adding a list makes them
> serve *only* the listed models, so the first pool member on such a key also writes the provider's default
> models into the list. The review dialog warns you when this happens.

## Install

### From the plugin store (once listed)

Console → **Plugin Store** → *Pools and Aliases* → Install. Then enable it (see below).

### Manually

Download the zip for your platform from [Releases](https://github.com/ngvoicu/cpa-pools-and-aliases/releases)
and put the library from inside it into your plugins directory. Or build it yourself:

```bash
make install PLUGINS_DIR=~/.cli-proxy-api/plugins
```

### Enable

```yaml
plugins:
  enabled: true
  dir: "~/.cli-proxy-api/plugins"
  configs:
    cpa-pools-and-aliases:
      enabled: true
```

Restart CPA. **Pools and Aliases** then appears under *Plugins* in the console sidebar. The page also
opens directly at `http://<cpa>/v0/resource/plugins/cpa-pools-and-aliases/ui`.

## The management key

The page calls the normal `/v8/management` API, so it needs your management key:

- If you logged in to the console with **Remember password**, the page reuses that key automatically.
- Otherwise it asks once. The key stays in this browser tab, or on the device if you tick
  *Remember on this device*.

The page itself is a static resource with no secrets in it. CPA serves plugin resources without
authentication, and every write still requires the management key.

## Build

Rust (stable). The page in `web/index.html` is embedded into the library at compile time.

```bash
make build      # dist/cpa-pools-and-aliases.{dylib,so,dll}
make test
make install PLUGINS_DIR=~/.cli-proxy-api/plugins
make package TARGET=aarch64-apple-darwin GOOS=darwin GOARCH=arm64   # release zip
```

After `make install`, restart CPA so it loads the new library. To update a running CPA **without a restart**
(a restart drops session affinity, and with it your prompt caches), install a versioned copy and pin it:

```bash
cp dist/cpa-pools-and-aliases.dylib ~/.cli-proxy-api/plugins/cpa-pools-and-aliases-v0.5.1.dylib
# then in the CPA config:  plugins.configs.cpa-pools-and-aliases.version: "0.5.1"
```

CPA hot-reloads the plugin as soon as it sees the config change. The plugin store does the same.

## Release

1. Bump `version` in `Cargo.toml` (for example `0.3.0`) and commit.
2. `git tag v0.3.0 && git push --tags`

GitHub Actions builds darwin/arm64, darwin/amd64, linux/amd64, linux/arm64 and windows/amd64. It
then publishes the release with the asset layout the CPA plugin store expects:

```
cpa-pools-and-aliases_0.2.0_darwin_arm64.zip   (contains cpa-pools-and-aliases.dylib at the root)
…
checksums.txt                                   (sha256sum format)
```

## Publishing to the CPA plugin store

The store is just a `registry.json` in
[router-for-me/CLIProxyAPI-Plugins-Store](https://github.com/router-for-me/CLIProxyAPI-Plugins-Store).
CPA reads your repository's **latest GitHub release** to install and update the plugin, so you only
need to submit the entry once:

1. Publish a release (see above) so the zips and `checksums.txt` exist.
2. Fork the store repo and add the contents of [`store/registry-entry.json`](store/registry-entry.json)
   to the `plugins` array in `registry.json`.
3. Open a PR that includes the repository URL, the release tag (`v0.1.0`), a link to the release
   assets, and a one-line description.

After it's merged, new releases ship automatically. There's nothing to change in the store.

## License

MIT
