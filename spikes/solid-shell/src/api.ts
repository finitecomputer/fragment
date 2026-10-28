export type Kind = "app" | "brain" | "skills" | "agent" | "chat" | "other";
export type Role = "owner" | "editor" | "viewer";
export interface Fragment {
  name: string;
  title?: string;
  kind: Kind;
  role: Role;
  picture?: string;
  dock?: boolean;
  archived?: boolean;
  sharing?: {
    visibility: "public" | "link" | "members";
    guests: number;
    invites: number;
  };
}
export interface Address {
  address: { label: string; url: string } | null;
  suffix: string;
  maxLength: number;
}
export interface Status {
  urls: { canonical: string };
  page?: { at: number };
}
export interface Candidate {
  png: string;
}
export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}
export async function api<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const response = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: {
      "x-fragment-shell": "1",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const value = await response.json();
  if (!response.ok)
    throw new ApiError(
      response.status,
      value.error,
      value.message ?? response.statusText,
    );
  return value as T;
}
export const pathOf = (name: string) => `/api/f/${encodeURIComponent(name)}`;
export const displayName = (fragment: Fragment) =>
  fragment.title || fragment.name.replace(/--[a-km-np-z2-9]{4}$/, "");
export const isApp = (fragment: Fragment) =>
  !["chat", "agent", "skills"].includes(fragment.kind);
export const canEdit = (fragment: Fragment) =>
  fragment.role === "owner" || fragment.role === "editor";

export async function rename(fragment: Fragment, title: string) {
  const path = pathOf(fragment.name);
  const manifest = await api<{ meta?: { title?: string } }>(
    "GET",
    `${path}/manifest`,
  );
  manifest.meta = { ...manifest.meta, title };
  await api("POST", `${path}/files`, {
    message: "rename app",
    files: [
      { path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" },
    ],
  });
  await api("POST", `${path}/deploy`, {});
}
