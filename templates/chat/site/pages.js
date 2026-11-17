// A channel page's `more` (docs/api.md): whether records remain past it in
// the direction read. A page ends at its bytes as well as its count, so a
// short page is the last only when it says so; a page from an older
// release says nothing, and there a page shorter than `asked` was the last
// (a page open across a rollback still pages on).
export function moreOf(page, asked) {
  if (typeof page.more === "boolean") return page.more;
  return (page.records?.length ?? 0) >= asked;
}
