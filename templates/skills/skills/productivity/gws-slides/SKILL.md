---
name: gws-slides
description: "Google Slides: Read and write presentations."
license: Apache-2.0
metadata:
  version: 0.22.5
  openclaw:
    category: "productivity"
    requires:
      bins:
        - gws
    cliHelp: "gws slides --help"
---

# slides (v1)

> **PREREQUISITE:** Load `skill_view("google-workspace-finite")` if not already loaded for the connected account, authentication, global flags, and security rules. Follow its connection check before the first Google call.

```bash
gws slides <resource> <method> [flags]
```

## API Resources

### presentations

  - `batchUpdate` — Applies one or more updates to the presentation. Each request is validated before being applied. If any request is not valid, then the entire request will fail and nothing will be applied. Some requests have replies to give you some information about how they are applied. Other requests do not need to return information; these each return an empty reply. The order of replies matches that of the requests.
  - `create` — Creates a blank presentation using the title given in the request. If a `presentationId` is provided, it is used as the ID of the new presentation. Otherwise, a new ID is generated. Other fields in the request, including any provided content, are ignored. Returns the created presentation.
  - `get` — Gets the latest version of the specified presentation.
  - `pages` — Operations on the 'pages' resource

## Discovering Commands

Inspect each method's schema before first use; reuse it for subsequent calls:

```bash
# Browse resources and methods
gws slides --help

# Inspect a method's required params, types, and defaults
gws schema slides.<resource>.<method>
```

Use `gws schema` output to build your `--params` and `--json` flags.

## Efficient edits

For an existing deck, read the affected slides first and reuse their object
IDs. Group related changes in one `presentations.batchUpdate`; use the
method's `fields` parameter to limit readback to the content you need.
After editing, read back the changed slides and inspect their rendering
with `presentations.pages.getThumbnail` or the browser before delivering
the presentation link.

`SERVICE_DISABLED` or `accessNotConfigured` means the deployment's Google
Cloud project needs the Slides API enabled;
report that to the platform operator instead of asking the owner to reconnect.

## Source

Adapted from Google Workspace CLI's [gws-slides v0.22.5](https://github.com/googleworkspace/cli/blob/705fb0ecac6f4249679958f6325b809b63fdde17/skills/gws-slides/SKILL.md), matching the image's pinned CLI. Apache-2.0; see [LICENSE](LICENSE).
Fragment changes: authentication points to `google-workspace-finite`, schema
discovery is reused, and batching, readback, connection-error guidance,
license metadata and this source notice are added.

