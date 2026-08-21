# Export assets — vendored from upstream

Source: `@earendil-works/pi-coding-agent` npm package (MIT license),
`dist/core/export-html/` — the HTML export templates used by
`GET /api/sessions/:id/export` (upstream pi-web's export route).

- Version: **0.84.1** (pinned in this repo's `package.json`, the same
  version pi-web's frontend is synced against)
- Files: `template.html`, `template.css`, `template.js`,
  `vendor/marked.min.js`, `vendor/highlight.min.js` — copied verbatim,
  no modifications (`ansi-to-html.js` / `tool-renderer.js` are
  server-side helpers for the TUI pre-render path and are not
  referenced by the standalone template)
- Embedded at compile time via `include_str!` from `src/api/export.rs`

## Refresh when tracking upstream

Bump `@earendil-works/pi-coding-agent` in `package.json`, then:

```bash
npm pack @earendil-works/pi-coding-agent@<version>
tar xzf <pkg>.tgz
cp package/dist/core/export-html/{template.html,template.css,template.js} .
cp package/dist/core/export-html/vendor/{marked.min.js,highlight.min.js} vendor/
```

Two details the export module relies on (verify after a refresh):

1. `template.js` is inlined through JS `String.replace`, so the asset
   intentionally contains `$$` (collapses to `$`) and
   `highlight.min.js` contains one `$&` (upstream bug: substitutes the
   placeholder literal into the output). `export.rs` implements the
   JS replacement-string semantics to reproduce this byte-for-byte.
2. The deep-chain stack-overflow patches in `export.rs::patch_export_html`
   match exact fragments of `template.js`; a template change can break
   the match (the patch then fails loudly with `Failed to patch ...`,
   mirroring upstream's `replaceRequired`).
