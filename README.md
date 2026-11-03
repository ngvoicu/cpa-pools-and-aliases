# Pools and Aliases for CLIProxyAPI

<img src="assets/logo.svg" width="64" align="right" alt="">

A [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) (CPA) plugin that adds a page to the
management console for setting **model aliases for every subscription and API key in one place**.

Every source names the same model differently: Devin has `devin/claude-opus-5-5`, Claude Code
has `claude-opus-5-5`, an API key has something else again. Give them all the alias `opus` and CPA
treats them as one pool. Ask for `opus` and your Routing tiers (Preferred → Fallback 1 → …) pick
which credential serves it.

```
alias      All Claude accounts   gabriel@… (Claude)   devin-user (Devin)       claude-1 (API key)
opus       —                     claude-opus-5-5      devin/claude-opus-5-5    claude-opus-5-5
sonnet     claude-sonnet-5-5     ↳ inherited          devin/claude-sonnet-5-5  —
workers    —                     —                    devin/swe-2              …
```

## What it does (and doesn't)

- **It only writes CPA's own aliases.** There's no router, no scheduler and no request interception.
  The plugin binary just serves the page. Disable or remove the plugin and every alias you made keeps
  working.
- Rows are aliases and columns are sources. Each cell is the upstream model that source maps the alias to.
- The **auto** button fills empty cells with the newest model whose name contains the alias.
- **keep originals** (CPA's `fork`) keeps the real model names visible next to the alias. Turn it off
  to hide them on that account.
- Every save shows a review of the exact changes first.

Where each column is saved:

| Column | Saved to | API used |
|---|---|---|
| **Account** (one OAuth login) | `model_aliases` in that credential's JSON file | `PATCH /v8/management/credentials/fields` |
| **All *X* accounts** (global) | `oauth.model-alias.<provider>` in config | `PUT /v8/management/config/oauth/model-alias` |
| **API key** | `api-keys.<provider>[group](.keys[i]).models[]` | `PUT /v8/management/config/api-keys/<provider>` |

An alias set on a single account takes priority over a global alias with the same name. CPA does
this, not the plugin. Inherited cells show as `↳ model`.

> **API keys without a model list.** A key with no `models` list serves every default model of its
> provider. Adding a list makes it serve *only* the listed models, so the first alias you add to such a
> key also writes the provider's default models into the list. That way the key keeps serving them.
> The review dialog warns you when this happens.

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

The plugin is plain C11 with no dependencies, plus a `python3` step that embeds `web/index.html`.

```bash
make build                          # dist/cpa-pools-and-aliases.{dylib,so,dll}
make package                        # dist/cpa-pools-and-aliases_<version>_<goos>_<goarch>.zip
make package GOOS=darwin GOARCH=amd64
make package GOOS=windows GOARCH=amd64 CC=x86_64-w64-mingw32-gcc
```

UI work: edit `web/index.html`, then run `make build install` and restart CPA. You can also open the file
straight in a browser. It then asks for the CPA URL, which needs management access allowed from
that origin.

## Release

1. Bump `VERSION` (for example `0.2.0`) and commit.
2. `git tag v0.2.0 && git push --tags`

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
