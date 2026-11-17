# Connection rows

One file per connection a deployment may offer: `<name>.jsonc`, the
provider's catalog row (`fragment_core::catalog`). A deployment names a row
in its `providers` and never copies it, so the row a release was tested
with is the row it deploys, and a fix here reaches every deployment
(`devstack::rows`):

```jsonc
{ "row": "linear", "key": "<the client secret's store name>", "client_id": "<the app's client id>" }
```

- `key` and `client_id` are the deployment's two facts, both or neither:
  the name, in the Secrets Store, of the OAuth client's secret, and the
  client's id. Named with neither (`{ "row": "google" }`, as dev and the
  e2e offer Google), a row is offered through Pipes without its
  browser-bound connect.
- Any other field beside `row` replaces the row's: a deployment's own
  `note`, or its `accounts_max`. Never its `name` or `kind`.
- A config may name a row before its setup is done. While the store lacks
  its secret, and the live deployment does not offer it, a deploy leaves
  it out with a warning and offers it on the first deploy after the secret
  is set (docs/secrets.md, The Secrets Store).

A row holds none of a deployment's facts. A connection is added here as two
things, and nothing else is edited (FIN-509):

1. **Its row**, `deploy/providers/<name>.jsonc`: its hosts, where its token
   goes, its OAuth endpoints and scopes, `title` (what people call it) and
   `scope_names` (each scope it asks for, for people: settings shows a
   granted scope by its name here, and no other).
2. **Its skill**, `templates/skills/skills/<category>/<name>-finite/SKILL.md`,
   whose frontmatter names it (`metadata: fragment: connection: <name>`):
   the provider's addresses and quirks, as calls of `fragment api <name>`.
   Docs only: no script, no helper.

No platform code, shell file or list names a connection. One test holds
every row to this shape
(`xtask`'s `each_connection_row_loads_names_its_scopes_and_has_a_skill`),
and one every skill (`fragment-templates`'
`the_skills_template_is_the_managed_set`).
