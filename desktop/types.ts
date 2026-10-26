export type Fragment = {
  name: string;
  kind: "chat" | "app" | "agent" | "brain" | "skills" | string;
  title?: string;
  role?: string;
  agents?: string[];
  picture?: string;
  preview?: string;
  previewAt?: number;
  archived?: boolean;
  dock?: boolean;
  unread?: number;
  listed?: Listed[];
};

export type Listed = {
  channel: string;
  principal: string;
  kind: string;
  at: number;
  fields?: { asks?: string };
};

export type Agent = {
  identity: string;
  fragment: string;
  phase?: string;
};

export type Computer = {
  computer: string;
  phase?: string;
  version?: string;
  latest?: string;
  image?: string;
  why?: string;
  alwaysOn?: boolean;
  agents?: Agent[];
  notices?: ComputerNotice[];
};

export type ComputerNotice = {
  kind: string;
  why?: string;
  since?: number;
  save?: number;
  savedAt?: number;
  stopsAt?: number;
  life?: number;
  version?: string;
};

export type Invite = {
  fragment: string;
  title: string;
  from?: string;
  role: string;
};

export type Person = {
  id: string;
  email?: string;
  picture?: string;
  subjects?: { email?: string }[];
};

export type SearchResults = {
  fragments?: Fragment[];
  messages?: { fragment: string; snippet: string; at: number; seq: number }[];
};

export type Workspace = {
  me: Person;
  guest: boolean;
  fragments: Fragment[];
  computer: Computer | null;
  invites: Invite[];
};

export const HOST = "https://finite.chat";
export const UNTITLED = "New chat";

export function labelOf(name: string): string {
  return name.replace(/--[a-km-np-z2-9]{4}$/, "");
}

export function titleOf(fragment: Fragment | undefined): string {
  return fragment?.title || (fragment ? labelOf(fragment.name) : "");
}

export function newest(a: Fragment, b: Fragment): number {
  return (b.previewAt ?? Number.POSITIVE_INFINITY) - (a.previewAt ?? Number.POSITIVE_INFINITY) || a.name.localeCompare(b.name);
}
