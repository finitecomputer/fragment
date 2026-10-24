---
name: gws-docs
description: "Read and write Google Docs."
license: Apache-2.0
metadata:
  version: 0.22.5
  openclaw:
    category: "productivity"
    requires:
      bins:
        - gws
    cliHelp: "gws docs --help"
---

# docs (v1)

> **PREREQUISITE:** Load `skill_view("google-workspace-finite")` if not already loaded for the connected account, authentication, global flags, and security rules. Follow its connection check before the first Google call.

```bash
gws docs <resource> <method> [flags]
```

## Helper Commands

| Command | Description |
|---------|-------------|
| `+write` | Append text to a document; inspect with `gws docs +write --help` |

## API Resources

### documents

  - `batchUpdate` — Applies one or more updates to the document. Each request is validated before being applied. If any request is not valid, then the entire request will fail and nothing will be applied. Some requests have replies to give you some information about how they are applied. Other requests do not need to return information; these each return an empty reply. The order of replies matches that of the requests.
  - `create` — Creates a blank document using the title given in the request. Other fields in the request, including any provided content, are ignored. Returns the created document.
  - `get` — Gets the latest version of the specified document.

## Discovering Commands

Before calling any API method, inspect it:

```bash
# Browse resources and methods
gws docs --help

# Inspect a method's required params, types, and defaults
gws schema docs.<resource>.<method>
```

Use `gws schema` output to build your `--params` and `--json` flags.

## Source

Adapted from Google Workspace CLI's [gws-docs v0.22.5](https://github.com/googleworkspace/cli/blob/705fb0ecac6f4249679958f6325b809b63fdde17/skills/gws-docs/SKILL.md), matching the image's pinned CLI. Apache-2.0; see [LICENSE](LICENSE).
Fragment changes: authentication points to `google-workspace-finite`, the append helper points to CLI help instead of an unshipped skill, and license metadata and this source notice are added.
