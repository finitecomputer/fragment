# Connection rows

One file per connection a deployment may offer: `<name>.jsonc`, the
provider's catalog row as a deployment's `providers` list takes it
(`deploy/example.jsonc`, `fragment_core::catalog`). To offer one, copy its
row into the deployment's config and fill its two blanks: `key` (the name,
in the Secrets Store, of the OAuth client's secret) and
`connection_oauth.client_id`.

A connection is added here as two things, and nothing else is edited
(FIN-509):

1. **Its row**, `deploy/providers/<name>.jsonc`: its hosts, where its token
   goes, its OAuth endpoints and scopes, `title` (what people call it) and
   `scope_names` (each scope it asks for, for people).
2. **Its skill**, `templates/skills/skills/<category>/<name>-finite/SKILL.md`,
   whose frontmatter names it (`metadata: fragment: connection: <name>`):
   the provider's addresses and quirks, as calls of `fragment api <name>`.
   Docs only: no script, no helper.

No platform code, shell file or list names a connection. One test holds
every row to this shape
(`xtask`'s `each_connection_row_loads_names_its_scopes_and_has_a_skill`),
and one every skill (`fragment-templates`'
`the_skills_template_is_the_managed_set`). Google's row is the example's
own, since dev and the e2e offer it.
