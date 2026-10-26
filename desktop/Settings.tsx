import { Scroll, Text, View, signal, openUrl } from "nsh";
import { Clipboard } from "nsh";
import type { Session } from "./session";
import type { Fragment, Workspace } from "./types";
import { Action, AgentAvatar, Section, SettingLine } from "./ui";

const money = (micros?: number) => micros === undefined ? "—" : (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD" });
const phases: Record<string, string> = { asleep: "Asleep", starting: "Starting", awake: "Awake", sleeping: "Going to sleep", wont_wake: "Won't start" };

export function Settings(props: {
  session: Session;
  workspace: Workspace;
  reload: () => Promise<void>;
  openApp: (name: string, path?: string) => void;
  openSpecial: (key: string, title: string, url: string) => void;
  signedOut: () => void;
}) {
  const details = signal<any>({});
  const busy = signal(false);
  const said = signal("");
  const computer = props.workspace.computer;
  const agents = computer?.agents ?? [];

  const load = async () => {
    const [ledger, seat, access, connections, exported] = await Promise.all([
      props.session.api("GET", "/api/ledger").catch(() => null),
      props.session.api("GET", "/api/seat").catch(() => null),
      props.session.api("GET", "/api/access").catch(() => null),
      props.session.api("GET", "/api/connections").catch(() => null),
      props.session.api("GET", "/api/export").catch(() => null),
    ]);
    details.set({ ledger, seat, access, connections, exported });
  };
  load();

  const act = async (fn: () => Promise<void>) => {
    if (busy()) return;
    busy.set(true);
    said.set("");
    try { await fn(); } catch (error) { said.set(String(error)); }
    busy.set(false);
  };

  const restart = () => act(async () => {
    if (!computer) return;
    await props.session.api("POST", `/api/computers/${computer.computer}/restart`, {});
    await props.reload();
    said.set("Your computer restarted.");
  });

  const checkout = (kind: "seat" | "pack") => act(async () => {
    const path = kind === "seat" ? "/api/billing/checkout" : "/api/billing/packs";
    const result = await props.session.api<{ url: string }>("POST", path, {});
    await openUrl(result.url);
  });

  return (
    <Scroll testID="settings-page" style={{ flex: 1 }} contentStyle={{ alignItems: "center" }}>
      <View style={{ width: "100%", maxWidth: 760, padding: 36, paddingBottom: 90 }}>
        <Text class="text-primary" style={{ fontSize: 30, fontWeight: "600", letterSpacing: -0.5 }}>Settings</Text>
        <Text class="text-secondary text-sm" style={{ marginTop: 5 }}>Manage your account, your computer, and the tools your agents use.</Text>
        {said() ? <Text class="text-secondary text-sm" style={{ marginTop: 16 }} testID="settings-status">{said()}</Text> : null}

        <Section title="Account">
          <SettingLine label="Signed in as" value={props.workspace.me.email || props.workspace.me.id} />
          <SettingLine label="Identity" value={props.workspace.me.id} mono />
          <SettingLine label="Session">
            <Action title="Sign out" label="Sign out" disabled={busy()} onPress={() => act(async () => {
              await props.session.api("POST", "/auth/logout", {}).catch(() => null);
              props.signedOut();
            })} />
          </SettingLine>
          <View style={{ flexDirection: "row", gap: 16, paddingVertical: 12 }}>
            <Text class="text-tertiary text-xs" onPress={() => openUrl(props.session.url("/privacy"))}>Privacy</Text>
            <Text class="text-tertiary text-xs" onPress={() => openUrl(props.session.url("/terms"))}>Terms</Text>
          </View>
        </Section>

        <Section title="Billing">
          <SettingLine label="Plan" value={props.workspace.guest ? "Guest" : details().seat?.seat?.kind === "seat_always_on" ? "Always-on seat" : "Seat"} />
          <SettingLine label="Available credit" value={money(details().ledger?.balance?.available ?? details().ledger?.available)} />
          <SettingLine label="Billing">
            <View style={{ flexDirection: "row", gap: 8 }}>
              {props.workspace.guest ? <Action title="Get a seat" label="Get a seat" onPress={() => checkout("seat")} /> : <Action title="Buy credit" label="Buy credit" onPress={() => checkout("pack")} />}
              {!props.workspace.guest ? <Action title="Manage billing" label="Manage" onPress={() => act(async () => {
                const result = await props.session.api<{ url: string }>("POST", "/api/billing/portal", {});
                await openUrl(result.url);
              })} /> : null}
            </View>
          </SettingLine>
        </Section>

        <Section title="Computer">
          <SettingLine label="State" value={computer ? phases[computer.phase ?? ""] ?? computer.phase : "Starts with your first agent"} />
          <SettingLine label="Version" value={computer?.version ?? computer?.latest ?? computer?.image ?? "—"} />
          {computer?.why ? <Text class="text-secondary text-sm" style={{ paddingVertical: 10 }}>{computer.why}</Text> : null}
          {computer ? <SettingLine label="Restart">
            <Action title="Restart computer" label="Restart computer" disabled={busy()} onPress={restart} />
          </SettingLine> : null}
          {agents.length ? <SettingLine label="Screens">
            <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
              {agents.map((agent) => <Action title={`Open ${agent.fragment}'s screen`} label="Open screen" onPress={() => act(async () => {
                const ticket = await props.session.api<{ url: string }>("POST", `/api/computers/${computer!.computer}/ports/6080/ticket`, { path: `/?agent=${encodeURIComponent(agent.fragment)}` });
                props.openSpecial(`screen:${agent.fragment}`, `${agent.fragment} · screen`, ticket.url);
              })} />)}
            </View>
          </SettingLine> : null}
        </Section>

        <Section title="Agents">
          {agents.length ? agents.map((agent) => (
            <View style={{ flexDirection: "row", alignItems: "center", gap: 12, paddingVertical: 12, borderBottomWidth: 1 }} class="border-border-strong">
              <AgentAvatar identity={agent.identity} picture={props.workspace.fragments.find((f) => f.name === agent.fragment)?.picture} size={30} />
              <Text class="text-primary" style={{ flex: 1, fontWeight: "600" }}>{props.workspace.fragments.find((f) => f.name === agent.fragment)?.title ?? agent.fragment}</Text>
              <Action title="Open agent profile" label="Open" onPress={() => props.openApp(agent.fragment)} />
            </View>
          )) : <Text class="text-secondary text-sm" style={{ padding: 12 }}>None yet.</Text>}
        </Section>

        <Section title="Sharing">
          <SettingLine label="Fragments" value={String(details().access?.fragments?.length ?? props.workspace.fragments.filter((f) => f.role === "owner").length)} />
          <Text class="text-secondary text-sm" style={{ paddingVertical: 12 }}>Open an app or chat’s menu to invite people, change access, or copy its link.</Text>
        </Section>

        <Section title="Skills">
          <SettingLine label="Managed skills" value="Your agents’ shared skills" />
          <SettingLine label="Library">
            <Action title="Open skills" label="Open" onPress={() => act(async () => {
              const existing = props.workspace.fragments.find((f) => f.kind === "skills");
              const skills = existing ?? await props.session.api<Fragment>("POST", "/api/fragments", { label: "skills", template: "skills" });
              props.openApp(skills.name, "/__files");
            })} />
          </SettingLine>
        </Section>

        <Section title="Connections">
          {(details().connections?.providers ?? []).length ? details().connections.providers.map((provider: any) => (
            <SettingLine label={provider.name ?? provider.provider ?? provider.kind} value={provider.state ?? "Not connected"} />
          )) : <Text class="text-secondary text-sm" style={{ padding: 12 }}>No connections are offered by this deployment.</Text>}
        </Section>

        <Section title="Build with the CLI">
          <Text class="text-secondary text-sm" style={{ paddingVertical: 12 }}>Pair the Fragment CLI with this account, then create and manage fragments from your terminal.</Text>
          <SettingLine label="Install">
            <Action title="Copy install command" label="Copy command" onPress={async () => {
              await Clipboard.writeText(`mkdir -p ~/.local/bin && curl -fsSL https://github.com/finitecomputer/fragment/releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz | tar -xzf - -C ~/.local/bin && fragment login --host ${props.session.host}`);
              said.set("Install command copied.");
            }} />
          </SettingLine>
        </Section>

        <Section title="Your data">
          <SettingLine label="Export" value={details().exported?.export?.status ?? "Everything in your account"} />
          <SettingLine label="Archive">
            <View style={{ flexDirection: "row", gap: 8 }}>
              <Action title="Export everything" label="Export everything" disabled={busy()} onPress={() => act(async () => {
                const result = await props.session.api("POST", "/api/export", {});
                details.update((old) => ({ ...old, exported: result }));
                said.set("Your export is building. Reload settings to check its progress.");
              })} />
              {details().exported?.export?.status === "ready" ? <Action title="Download export" label="Download" onPress={() => openUrl(props.session.url(`/api/export/${details().exported.export.id}/download`))} /> : null}
            </View>
          </SettingLine>
        </Section>
      </View>
    </Scroll>
  );
}
