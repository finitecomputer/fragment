// This page's posts, by the id they keep from composer through server record.
// The record is authoritative: posted is terminal, including when its socket
// event arrives before the HTTP response for the same post.
export class OutgoingPosts extends Map {
  transition(id, status, { message = null, record = null, error = null } = {}) {
    const before = this.get(id) ?? null;
    if (before?.status === "posted") return { post: before, retried: false };
    if (status === "posted") {
      if (!before || !record) return { post: before, retried: false };
      const post = { ...before, status, seq: record.seq, error: null };
      this.set(id, post);
      return { post, retried: before.status === "retrying" };
    }
    if (status === "posting") {
      const next = before?.status === "failed" || before?.status === "retrying" ? "retrying" : "sending";
      const post = { ...(before ?? message), status: next };
      this.set(id, post);
      return { post, retried: false };
    }
    if (status === "failed") {
      const post = { ...(before ?? message), status, error };
      this.set(id, post);
      return { post, retried: false };
    }
    throw new Error(`unknown outgoing post status: ${status}`);
  }

  record(record) {
    const said = JSON.stringify(record.body);
    for (const [id, post] of this) {
      if ((post.status !== "sending" && post.status !== "retrying") || JSON.stringify(post.body) !== said) continue;
      return this.transition(id, "posted", { record });
    }
    return { post: null, retried: false };
  }

  postId(seq) {
    for (const [id, post] of this) if (post.status === "posted" && post.seq === seq) return id;
    return null;
  }
}
