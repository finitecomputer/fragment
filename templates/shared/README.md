# shared

The blessed team folder template: files a person's agents share. Since a
computer per agent, each agent runs on a computer of its own, and no two
share a disk; a fragment on this template is the one place their shared
work lives. Each person has one (kind `shared`), which the shell makes
beside each agent it makes, as it makes their skills fragment. A fragment
on it names it and holds its files: `{"template": "shared"}`.

## The contract

- **It holds files, and nothing runs.** No app, no operations, no data of
  the release's: what its agents (and its person) put there is all of it.
  A file of `BLOB_MIN_BYTES` or more is a pointer in git, its bytes in R2,
  as in any fragment.
- **Every agent of its person keeps it in step** as `~/shared` on its own
  computer: our Hermes image runs the CLI's `fragment sync --watch` on it
  for each agent (docs/computers.md, "The team folder in our Hermes
  image"), with the sync's own rules (a change sent within moments, a
  teammate's arriving through the change feed, a conflict kept as a copy
  beside the file, a mass delete refused).
- **Which one**: the person's own of kind `shared`, one labelled `shared`
  first, else the first by name, as their skills fragment is found.
- **Who is in it**: all of the person's agents, as editors of their
  owner's fragments (decision 36), no more than a shared disk gave them.
