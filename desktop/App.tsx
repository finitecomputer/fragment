import { Clipboard, Image, Scroll, Storage, Text, View, WebView, colorScheme, effect, keymap, mount, openUrl, signal, type YNode } from "nsh";
import { Input } from "nsh/base";
import { Menu, Window } from "nsh/desktop";
import { Shell } from "finite-computer/native";
import { Session } from "./session";
import { Settings } from "./Settings";
import { Action, AgentAvatar, AppIcon, Empty, FragmentMark, Icon } from "./ui";
import { newest, titleOf, UNTITLED, type Fragment, type SearchResults, type Workspace } from "./types";
import wallpaper from "./assets/wallpaper.jpg";
import wallpaperDark from "./assets/wallpaper-dark.jpg";
import funnel from "./assets/funnel-sans-400.ttf";

Window.setTitle("Finite.Computer");
Window.setFrame({ size: { width: 1440, height: 900 } });

const APP_PROMPTS = [
  "Make a shared grocery list my family can add to from their phones",
  "Tell me on my phone as soon as … comes back in stock or drops in price",
  "Every morning, read these sites and send me a short summary: …",
  "Make a page for our trip to … where we log who paid for what and it shows who owes whom",
  "Build a habit tracker that shows my weekly streak for: …",
  "Make an RSVP page for my dinner on Saturday that tells me who's coming",
];

type Pane = { key: string; name: string; title: string; url: string; special?: boolean; path?: string };

function FragmentView(props: { session: Session; pane: Pane; register: (node: YNode) => void; message: (data: any, origin: string) => void }) {
  return (
    <WebView
      ref={(node: YNode) => props.register(node)}
      src={props.pane.url}
      style={{ flex: 1, minHeight: 0 }}
      onMessage={(event) => props.message(JSON.parse(event.data), "")}
      onError={(event) => console.error(`Fragment ${props.pane.name}: ${event.message}`)}
    />
  );
}

function App() {
  const session = new Session(Shell.host());
  const workspace = signal<Workspace | null>(null);
  const connecting = signal(true);
  const signedOut = signal(false);
  const fixture = signal(false);
  const current = signal("");
  const page = signal<"chat" | "settings">("chat");
  const leftOpen = signal(true);
  const rightOpen = signal(false);
  const panes = signal<Pane[]>([]);
  const notice = signal("");
  const busy = signal(false);
  const searchOpen = signal(false);
  const query = signal("");
  const found = signal<SearchResults>({});
  const searchStatus = signal("");
  const renameOpen = signal(false);
  const renameName = signal("");
  const renameValue = signal("");
  const shareName = signal("");
  const frameNodes = new Map<string, YNode>();
  const pendingCompose = new Map<string, string>();
  let searchGeneration = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let pendingLaunch: { text: string; id: string; chat?: string } | null = null;

  const fragments = () => workspace()?.fragments ?? [];
  const byName = (name: string) => fragments().find((fragment) => fragment.name === name);
  const chats = () => fragments().filter((fragment) => fragment.kind === "chat" && !fragment.archived).sort(newest);
  const apps = () => fragments().filter((fragment) => (fragment.kind === "app" || fragment.kind === "brain") && !fragment.archived && fragment.dock !== false);
  const agents = () => new Map((workspace()?.computer?.agents ?? []).map((agent) => [agent.identity, agent]));
  const primary = () => {
    const owned = workspace()?.computer?.agents?.find((agent) => byName(agent.fragment)?.role === "owner");
    if (owned) return owned;
    const fragment = fragments().find((item) => item.kind === "agent" && item.role === "owner");
    return fragment ? { identity: "", fragment: fragment.name } : null;
  };
  const title = (name: string) => titleOf(byName(name)) || name;
  const activeFragment = () => byName(current());

  const load = async () => {
    if (fixture()) return;
    try {
      const next = await session.workspace();
      workspace.set(next);
      signedOut.set(false);
      connecting.set(false);
      if (!current() || !next.fragments.some((fragment) => fragment.name === current())) {
        current.set(next.fragments.find((fragment) => fragment.kind === "chat" && !fragment.archived)?.name ?? "");
      }
      notice.set("");
      const open = byName(current());
      if (open?.unread && page() === "chat") markSeen(open.name);
    } catch (error: any) {
      connecting.set(false);
      if (error?.status === 401) signedOut.set(true);
      else notice.set(error?.message ?? String(error));
    }
  };

  session.onChange = () => { load(); };
  session.onInvites = () => { load(); };
  session.onFragment = (name, data, origin) => {
    const node = frameNodes.get(name);
    if (!node || !data || typeof data !== "object") return;
    if (data.fragment === "open-launcher") searchOpen.set(true);
    if (data.fragment === "agents?") {
      const payload = {
        fragment: "agents",
        agents: (workspace()?.computer?.agents ?? []).map((agent) => ({
          ...agent,
          name: agent.fragment.replace(/--[a-km-np-z2-9]{4}$/, ""),
          title: title(agent.fragment),
          picture: byName(agent.fragment)?.picture,
          phase: workspace()?.computer?.phase,
        })),
      };
      node.postMessage(JSON.stringify({ desktopToFragment: true, payload, origin }));
      if (pendingCompose.has(name)) {
        const text = pendingCompose.get(name)!;
        pendingCompose.delete(name);
        node.postMessage(JSON.stringify({ desktopToFragment: true, payload: { fragment: "compose", text }, origin }));
      }
    }
    if (data.fragment === "alerts?" || data.fragment === "alerts") {
      node.postMessage(JSON.stringify({ desktopToFragment: true, origin, payload: { fragment: "alerts", nonce: data.nonce, state: "unsupported" } }));
    }
  };

  effect(() => {
    if (session.ready() && !fixture() && connecting()) load();
  });

  Storage.getItem("desktop.fixture").then(async (saved) => {
    if (saved) {
      workspace.set(JSON.parse(saved));
      fixture.set(true);
      connecting.set(false);
      current.set(JSON.parse(saved).fragments.find((f: Fragment) => f.kind === "chat")?.name ?? "");
      return;
    }
    const last = await Storage.getItem("desktop.current");
    if (last) current.set(last);
  }).catch(() => connecting.set(false));

  keymap.bind("mod+k", () => searchOpen.set(true));
  keymap.bind("mod+b", () => leftOpen.update((open) => !open));
  keymap.bind("alt+mod+b", () => rightOpen.update((open) => !open));

  const openChat = (name: string) => {
    if (!byName(name)) return;
    current.set(name);
    page.set("chat");
    searchOpen.set(false);
    notice.set("");
    Storage.setItem("desktop.current", name);
    markSeen(name);
  };

  const markSeen = (name: string) => {
    const fragment = byName(name);
    if (!fragment?.unread || fixture()) return;
    session.api("PUT", `/api/fragments/${name}/seen`).then(load).catch(() => {});
  };

  const openApp = (name: string, path = "/") => {
    const key = `app:${name}`;
    page.set("chat");
    rightOpen.set(true);
    searchOpen.set(false);
    const existing = panes().find((pane) => pane.key === key);
    if (existing) {
      if (path !== "/") panes.update((all) => all.map((pane) => pane.key === key ? { ...pane, path, url: session.fragmentUrl(name, path) } : pane));
      return;
    }
    panes.update((all) => [...all, { key, name, title: title(name), path, url: session.fragmentUrl(name, path) }]);
  };

  const openSpecial = (key: string, paneTitle: string, url: string) => {
    rightOpen.set(true);
    page.set("chat");
    if (!panes().some((pane) => pane.key === key)) panes.update((all) => [...all, { key, name: key, title: paneTitle, url, special: true }]);
  };

  const closePane = (key: string) => {
    panes.update((all) => all.filter((pane) => pane.key !== key));
    frameNodes.delete(key);
  };

  const newChat = async (identity?: string): Promise<string | null> => {
    const agent = agents().get(identity ?? primary()?.identity ?? "");
    if (!agent || fixture()) return null;
    const existing = chats().find((fragment) => fragment.role === "owner" && fragment.title === UNTITLED && !fragment.preview && fragment.agents?.includes(agent.identity));
    if (existing) { openChat(existing.name); return existing.name; }
    const taken = fragments().map((fragment) => fragment.name.replace(/--[a-km-np-z2-9]{4}$/, ""));
    const base = agent.fragment.replace(/--[a-km-np-z2-9]{4}$/, "");
    const made = await session.api<Fragment>("POST", "/api/fragments", { label: Shell.freeLabel(`${base}-chat`, taken), template: "chat", title: UNTITLED });
    await session.api("PUT", `/api/f/${made.name}/members/${agent.identity}`, { role: "editor" });
    await load();
    openChat(made.name);
    return made.name;
  };

  const startChat = async () => {
    const text = query().trim();
    if (!text || busy()) return;
    if (fixture()) { searchStatus.set("Sign in to start a chat."); return; }
    busy.set(true);
    searchStatus.set("Starting your chat…");
    try {
      if (!pendingLaunch || pendingLaunch.text !== text) pendingLaunch = { text, id: crypto.randomUUID() };
      pendingLaunch.chat ||= await newChat() ?? undefined;
      if (!pendingLaunch.chat) throw new Error("Your agent is not ready yet.");
      await session.api("POST", `/api/f/${pendingLaunch.chat}/channels/chat`, { id: pendingLaunch.id, body: { text } });
      openChat(pendingLaunch.chat);
      pendingLaunch = null;
      query.set("");
      found.set({});
      searchStatus.set("");
    } catch (error) {
      searchStatus.set(`Your message wasn’t sent. ${String(error)}`);
    }
    busy.set(false);
  };

  const updateSearch = (text: string) => {
    query.set(text);
    found.set({});
    searchStatus.set("");
    clearTimeout(searchTimer);
    const generation = ++searchGeneration;
    if (!text.trim()) return;
    searchTimer = setTimeout(async () => {
      try {
        const result = await session.search(text, Shell.searchHint(text));
        if (generation === searchGeneration) found.set(result);
      } catch (error) {
        if (generation === searchGeneration) searchStatus.set(String(error));
      }
    }, 150);
  };

  const rename = (name: string) => {
    renameName.set(name);
    renameValue.set(title(name));
    renameOpen.set(true);
  };

  const saveRename = async () => {
    const value = renameValue().trim();
    if (!value || busy()) return;
    busy.set(true);
    try {
      const name = renameName();
      const manifest = await session.api<any>("GET", `/api/f/${name}/manifest`);
      manifest.meta = { ...(manifest.meta ?? {}), title: value };
      await session.api("POST", `/api/f/${name}/files`, { message: "rename", files: [{ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" }] });
      await session.api("POST", `/api/f/${name}/deploy`, {});
      renameOpen.set(false);
      await load();
    } catch (error) { notice.set(String(error)); }
    busy.set(false);
  };

  const archive = async (name: string) => {
    const archived = !byName(name)?.archived;
    await session.api("PUT", `/api/fragments/${name}/archived`, { archived });
    if (archived) closePane(`app:${name}`);
    await load();
  };

  const compose = async (text: string) => {
    closePane("prompts");
    let name = current();
    if (page() === "settings" || byName(name)?.kind !== "chat") {
      const own = chats().find((chat) => chat.role === "owner" && chat.agents?.includes(primary()?.identity ?? ""));
      name = own?.name ?? await newChat() ?? "";
    }
    if (!name) return;
    openChat(name);
    const payload = { fragment: "compose", text };
    const node = frameNodes.get(name);
    if (node) node.postMessage(JSON.stringify({ desktopToFragment: true, payload }));
    else pendingCompose.set(name, text);
  };

  const showPrompts = () => {
    rightOpen.set(true);
    if (!panes().some((pane) => pane.key === "prompts")) panes.update((all) => [...all, { key: "prompts", name: "prompts", title: "Ask for an app", url: "", special: true }]);
  };

  const primaryMenu = () => {
    const agent = primary();
    if (!agent) return;
    Menu.popup([
      agent.identity && !workspace()?.guest ? { label: "New chat", onSelect: () => newChat(agent.identity).catch((error) => notice.set(String(error))) } : null,
      { label: "Rename…", onSelect: () => rename(agent.fragment) },
      { label: "Its profile", onSelect: () => openApp(agent.fragment) },
      workspace()?.computer ? { label: "Its screen", onSelect: async () => {
        try {
          const ticket = await session.api<{ url: string }>("POST", `/api/computers/${workspace()!.computer!.computer}/ports/6080/ticket`, { path: `/?agent=${encodeURIComponent(agent.fragment)}` });
          openSpecial(`screen:${agent.fragment}`, `${title(agent.fragment)} · screen`, ticket.url);
        } catch (error) { notice.set(String(error)); }
      } } : null,
    ]);
  };

  const chatMenu = (name: string) => Menu.popup([
    { label: "Rename…", onSelect: () => rename(name) },
    { label: byName(name)?.role === "owner" ? "Share…" : "People…", onSelect: () => shareName.set(name) },
    { type: "separator" },
    { label: byName(name)?.archived ? "Unarchive" : "Archive", onSelect: () => archive(name).catch((error) => notice.set(String(error))) },
  ]);

  const appMenu = (pane: Pane) => Menu.popup([
    { label: "Copy link", onSelect: async () => {
      const status = await session.api<any>("GET", `/api/f/${pane.name}/status`);
      await Clipboard.writeText(`${status.canonicalUrl ?? status.url}${pane.path ?? "/"}`);
    } },
    pane.name.startsWith("app:") || byName(pane.name)?.role !== "owner" ? null : { label: "Share…", onSelect: () => shareName.set(pane.name) },
    { label: "Open in browser", onSelect: () => openUrl(session.fragmentUrl(pane.name, pane.path ?? "/", false)) },
    { type: "separator" },
    { label: "Close", onSelect: () => closePane(pane.key) },
  ]);

  const Sidebar = () => (
    <View class="bg-window" style={{ width: 264, margin: 8, marginRight: 0, borderRadius: 12, padding: 12, gap: 4 }}>
      <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between", height: 38 }}>
        <Text class="text-primary" style={{ fontSize: 15, fontWeight: "600", letterSpacing: -0.25 }}>Finite.Computer</Text>
        <Action icon="plus" title="Search or start a new chat" testID="open-launcher" onPress={() => searchOpen.set(true)} />
      </View>
      {primary() ? (
        <View class="bg-sidebar rounded-full" style={{ flexDirection: "row", alignItems: "center", gap: 10, padding: 9, marginVertical: 7 }} onPress={primaryMenu} role="button" accessibilityLabel="Your agent’s menu" testID="agent-menu">
          <AgentAvatar identity={primary()?.identity} picture={byName(primary()!.fragment)?.picture} size={27} />
          <Text class="text-primary text-sm font-semibold" style={{ flex: 1 }} numberOfLines={1}>{title(primary()!.fragment)}</Text>
        </View>
      ) : null}
      {primary()?.identity && !workspace()?.guest ? <Action icon="chat" label="New chat" title="New chat" testID="new-chat" onPress={() => newChat().catch((error) => notice.set(String(error)))} /> : null}
      {workspace()?.invites.length ? (
        <Scroll style={{ maxHeight: 150 }}>
          <Each of={workspace()!.invites}>{(invite) => (
            <View class="border-border-strong rounded-xl" style={{ borderWidth: 1, padding: 10, marginBottom: 6, gap: 4 }}>
              <Text class="text-primary text-sm font-semibold" numberOfLines={1}>{invite().title}</Text>
              <Text class="text-secondary text-xs">{invite().from ?? "Someone"} invited you to {invite().role === "editor" ? "edit" : "view"}</Text>
              <View style={{ flexDirection: "row", gap: 6 }}>
                <Action label="Accept" title="Accept invite" onPress={() => session.api("POST", `/api/invites/${invite().fragment}/accept`, {}).then(() => load()).then(() => openApp(invite().fragment))} />
                <Action label="Decline" title="Decline invite" onPress={() => session.api("POST", `/api/invites/${invite().fragment}/decline`, {}).then(load)} />
              </View>
            </View>
          )}</Each>
        </Scroll>
      ) : null}
      <Scroll testID="chat-list" style={{ flex: 1 }} contentStyle={{ gap: 2 }}>
        <Each of={chats()}>{(chat) => (
          <View
            testID={`chat-${chat().name}`}
            role="button"
            accessibilityLabel={titleOf(chat())}
            onPress={() => openChat(chat().name)}
            class={`rounded-lg ${current() === chat().name && page() === "chat" ? "bg-active" : ""} active:bg-hover`}
            style={{ minHeight: 34, paddingHorizontal: 10, paddingVertical: 7, flexDirection: "row", alignItems: "center", gap: 8 }}
          >
            <Text class={`${current() === chat().name ? "text-primary" : "text-secondary"} text-sm`} style={{ flex: 1 }} numberOfLines={1}>{titleOf(chat())}</Text>
            {chat().unread && current() !== chat().name ? <View class="bg-accent" style={{ borderRadius: 10, minWidth: 20, paddingHorizontal: 5 }}><Text class="text-on-accent text-xs" style={{ textAlign: "center" }}>{chat().unread! >= 100 ? "99+" : chat().unread}</Text></View> : null}
          </View>
        )}</Each>
        {!chats().length ? <Text class="text-tertiary text-xs" style={{ padding: 8 }}>{workspace()?.guest ? "Chats shared with you show here" : "Your chats will appear here"}</Text> : null}
      </Scroll>
      <Action icon="settings" label="Settings" title="Settings" testID="settings" active={page() === "settings"} onPress={() => page.set("settings")} />
    </View>
  );

  const ComputerNotices = () => (
    <Each of={workspace()?.computer?.notices ?? []}>{(item) => (
      <View class="bg-card border-border-strong rounded-xl" style={{ borderWidth: 1, padding: 12, marginHorizontal: 14, marginBottom: 8, flexDirection: "row", alignItems: "center", gap: 12 }}>
        <View style={{ flex: 1, gap: 3 }}>
          <Text class="text-primary text-sm font-semibold">{item().kind === "wont_wake" ? "Your computer won’t start" : item().kind === "unsaved" ? "Your computer can’t save right now" : item().kind === "update" ? "A computer update is ready" : "Your computer went back to an earlier save"}</Text>
          <Text class="text-secondary text-xs">{item().why ?? (item().kind === "update" ? "Restart to use the latest version. Your chats and saved work stay here." : "Your chats are all here. Restart to try again.")}</Text>
        </View>
        {item().kind === "update" ? <Action label="Later" title="Remind me later" onPress={() => session.api("POST", `/api/computers/${workspace()!.computer!.computer}/notices/later`, { version: item().version }).then(load)} /> : null}
        {item().kind === "went_back" ? <Action label="Got it" title="Dismiss notice" onPress={() => session.api("POST", `/api/computers/${workspace()!.computer!.computer}/notices/seen`, { life: item().life }).then(load)} /> : null}
        {item().kind !== "went_back" ? <Action label="Restart" title="Restart computer" onPress={() => session.api("POST", `/api/computers/${workspace()!.computer!.computer}/restart`, {}).then(load)} /> : null}
      </View>
    )}</Each>
  );

  const Chat = () => (
    <View class="bg-sidebar" style={{ flex: 1, minWidth: 0 }}>
      <View style={{ height: 66, flexDirection: "row", alignItems: "center", paddingHorizontal: 14, gap: 10 }}>
        {!leftOpen() ? <Action icon="desktop" title="Show sidebar" onPress={() => leftOpen.set(true)} /> : null}
        <View style={{ flex: 1, alignItems: "center" }}>
          <Text class="text-primary text-sm font-semibold" numberOfLines={1}>{page() === "settings" ? "Settings" : activeFragment() ? titleOf(activeFragment()) : "Finite.Computer"}</Text>
        </View>
        {page() === "chat" && current() ? <Action icon="more" title="Chat options" testID="chat-options" onPress={() => chatMenu(current())} /> : null}
        {page() === "chat" ? <Action icon="desktop" title="Toggle desktop" testID="toggle-desktop" active={rightOpen()} onPress={() => rightOpen.set(!rightOpen())} /> : null}
      </View>
      <ComputerNotices />
      {notice() ? <View style={{ padding: 20, alignItems: "center" }}><Text class="text-secondary text-sm">{notice()}</Text></View> : null}
      {page() === "settings" && workspace() ? <Settings session={session} workspace={workspace()!} reload={load} openApp={openApp} openSpecial={openSpecial} signedOut={() => { workspace.set(null); signedOut.set(true); }} /> : null}
      {page() === "chat" && current() && !fixture() ? <FragmentView session={session} pane={{ key: current(), name: current(), title: title(current()), url: session.fragmentUrl(current()) }} register={(node) => { frameNodes.set(current(), node); }} message={(data, origin) => session.onFragment?.(current(), data.data ?? data, data.origin ?? origin)} /> : null}
      {page() === "chat" && current() && fixture() ? <Empty title={title(current())} body="This is a native preview. Chat content is rendered by Fragment in a native web view when connected." /> : null}
      {page() === "chat" && !current() ? <Empty title={workspace()?.guest ? "Nothing open" : "Your chats will appear here"} body={workspace()?.guest ? "Accept an invite or search for something shared with you." : "Start a new chat with your agent from the + button."} /> : null}
    </View>
  );

  const PromptPane = () => (
    <Scroll style={{ flex: 1 }} contentStyle={{ padding: 18, gap: 10 }}>
      <Text class="text-secondary text-sm">{primary() ? title(primary()!.fragment) : "Your agent"} makes an app from a message. Pick a starting point, make it yours, then send it.</Text>
      {APP_PROMPTS.slice(0, 3).map((prompt) => <View class="bg-card border-border-strong rounded-xl" style={{ borderWidth: 1, padding: 12 }} onPress={() => compose(prompt)} role="button"><Text class="text-primary text-sm">{prompt}</Text></View>)}
    </Scroll>
  );

  const Desktop = () => (
    <View style={{ width: 440, overflow: "hidden", borderTopLeftRadius: 16, borderBottomLeftRadius: 16 }}>
      <Image src={colorScheme() === "dark" ? wallpaperDark : wallpaper} resizeMode="cover" style={{ position: "absolute", width: "100%", height: "100%" }} />
      <View style={{ flex: 1, padding: panes().length ? 8 : 0, gap: 8 }}>
        <Each of={panes()}>{(pane) => (
          <View class="bg-window rounded-xl" style={{ flex: 1, minHeight: 0, overflow: "hidden" }}>
            <View class="bg-sidebar border-border-strong" style={{ height: 38, borderBottomWidth: 1, flexDirection: "row", alignItems: "center", paddingHorizontal: 6, gap: 5 }}>
              {pane().special ? <Icon name="computer" class="text-secondary" /> : <AppIcon name={pane().name} />}
              <Text class="text-primary text-xs font-semibold" style={{ flex: 1 }} numberOfLines={1}>{pane().title}</Text>
              {!pane().special ? <Action icon="reload" title="Reload" onPress={() => panes.update((all) => all.map((item) => item.key === pane().key ? { ...item, url: `${item.url.split("#")[0]}#${Date.now()}` } : item))} /> : null}
              {!pane().special ? <Action icon="more" title="More" onPress={() => appMenu(pane())} /> : null}
              <Action icon="close" title="Close window" onPress={() => closePane(pane().key)} />
            </View>
            {pane().key === "prompts" ? <PromptPane /> : pane().special ? <WebView src={pane().url} style={{ flex: 1 }} /> : !fixture() ? <FragmentView session={session} pane={pane()} register={(node) => frameNodes.set(pane().name, node)} message={(data, origin) => session.onFragment?.(pane().name, data.data ?? data, data.origin ?? origin)} /> : <Empty title={pane().title} body="Your app opens here in its own native window." />}
          </View>
        )}</Each>
        {!panes().length ? <View style={{ flex: 1, alignItems: "center", justifyContent: "center", padding: 24, gap: 8 }}><Text style={{ color: "white", fontSize: 17, fontWeight: "600" }}>Your apps land here</Text><Text style={{ color: "white", fontSize: 13 }}>Ask your agent for one from Add.</Text></View> : null}
      </View>
      <View style={{ flexDirection: "row", flexWrap: "wrap", justifyContent: "center", alignSelf: "center", padding: 8, gap: 7, maxWidth: "100%" }}>
        <Each of={apps()}>{(app) => (
          <View onPress={() => openApp(app().name)} role="button" accessibilityLabel={titleOf(app())} style={{ width: 52, alignItems: "center", gap: 4 }}>
            <AppIcon name={app().name} size={40} />
            <Text style={{ color: "white", fontSize: 9 }} numberOfLines={1}>{titleOf(app())}</Text>
          </View>
        )}</Each>
        {!workspace()?.guest ? <View onPress={showPrompts} role="button" accessibilityLabel="Ask for an app" testID="add-app" style={{ width: 52, alignItems: "center", gap: 4 }}><View style={{ width: 40, height: 40, borderRadius: 11, borderWidth: 1, borderColor: "#ffffff88", alignItems: "center", justifyContent: "center" }}><Text style={{ color: "white", fontSize: 24 }}>+</Text></View><Text style={{ color: "white", fontSize: 9 }}>Add</Text></View> : null}
      </View>
    </View>
  );

  const Search = () => (
    <View style={{ position: "absolute", left: 0, top: 0, right: 0, bottom: 0, zIndex: 30 }}>
      <Image src={colorScheme() === "dark" ? wallpaperDark : wallpaper} resizeMode="cover" style={{ position: "absolute", width: "100%", height: "100%", opacity: 0.96 }} />
      <View class="bg-sidebar border-border-strong" style={{ width: 620, maxWidth: "90%", maxHeight: 560, alignSelf: "center", marginTop: 160, borderWidth: 1, borderRadius: 30, overflow: "hidden" }}>
        <View style={{ flexDirection: "row", alignItems: "center", paddingLeft: 20, paddingRight: 10, gap: 8 }}>
          <Input testID="workspace-search" autoFocus value={query()} onChange={updateSearch} onSubmit={startChat} placeholder="Ask anything, or find a chat or app…" class="text-primary placeholder:text-secondary" style={{ flex: 1, height: 58, fontSize: 17 }} />
          <Action icon="plus" title="Start a new chat" disabled={!query().trim() || busy()} onPress={startChat} />
          <Action icon="close" title="Close search" onPress={() => searchOpen.set(false)} />
        </View>
        {query().trim() ? <View class="border-border-strong" style={{ borderTopWidth: 1 }} /> : null}
        <Scroll style={{ maxHeight: 410 }} contentStyle={{ padding: 8 }}>
          {query().trim() && primary()?.identity ? <View class="rounded-lg active:bg-hover" style={{ padding: 10, flexDirection: "row", gap: 10 }} onPress={startChat} role="button"><Icon name="chat" class="text-secondary" /><View style={{ flex: 1 }}><Text class="text-primary text-sm font-semibold">Start a new chat with {title(primary()!.fragment)}</Text><Text class="text-secondary text-xs" numberOfLines={2}>{query()}</Text></View></View> : null}
          {found().fragments?.length ? <Text class="text-secondary text-xs" style={{ margin: 10, textTransform: "uppercase" }}>Chats and apps</Text> : null}
          <Each of={found().fragments ?? []}>{(fragment) => <View class="rounded-lg active:bg-hover" style={{ padding: 10, flexDirection: "row", alignItems: "center", gap: 10 }} onPress={() => fragment().kind === "chat" ? openChat(fragment().name) : openApp(fragment().name)} role="button"><FragmentMark fragment={fragment()} agents={agents()} /><View style={{ flex: 1 }}><Text class="text-primary text-sm">{titleOf(fragment())}</Text>{fragment().preview ? <Text class="text-secondary text-xs" numberOfLines={2}>{fragment().preview}</Text> : null}</View>{fragment().archived ? <Text class="text-tertiary text-xs">Archived</Text> : null}</View>}</Each>
          {found().messages?.length ? <Text class="text-secondary text-xs" style={{ margin: 10, textTransform: "uppercase" }}>Messages</Text> : null}
          <Each of={found().messages ?? []}>{(message) => <View class="rounded-lg active:bg-hover" style={{ padding: 10 }} onPress={() => openChat(message().fragment)} role="button"><Text class="text-primary text-sm">{title(message().fragment)}</Text><Text class="text-secondary text-xs" numberOfLines={2}>{message().snippet}</Text></View>}</Each>
          {searchStatus() ? <Text class="text-secondary text-xs" style={{ padding: 12 }} testID="search-status">{searchStatus()}</Text> : null}
        </Scroll>
      </View>
    </View>
  );

  const Rename = () => (
    <View class="bg-scrim" style={{ position: "absolute", left: 0, top: 0, right: 0, bottom: 0, zIndex: 40, alignItems: "center", justifyContent: "center" }}>
      <View class="bg-card border-border-strong" style={{ width: 400, borderWidth: 1, borderRadius: 18, padding: 26, gap: 12 }}>
        <Text class="text-primary" style={{ fontSize: 21, fontWeight: "600" }}>{byName(renameName())?.kind === "agent" ? "Name your agent" : "Name this chat"}</Text>
        <Text class="text-secondary text-sm">A name for what you’re working on together.</Text>
        <Input testID="rename-input" autoFocus value={renameValue()} onChange={renameValue.set} onSubmit={saveRename} maxLength={48} class="text-primary bg-window border-border-strong" style={{ borderWidth: 1, borderRadius: 9, padding: 12, marginTop: 10 }} />
        <View style={{ flexDirection: "row", justifyContent: "flex-end", gap: 10, marginTop: 10 }}><Action label="Cancel" title="Cancel rename" onPress={() => renameOpen.set(false)} /><Action label="Save name" title="Save name" disabled={busy()} onPress={saveRename} /></View>
      </View>
    </View>
  );

  const Share = () => (
    <View class="bg-scrim" style={{ position: "absolute", left: 0, top: 0, right: 0, bottom: 0, zIndex: 40, alignItems: "center", justifyContent: "center" }}>
      <View class="bg-card rounded-xl" style={{ width: 520, height: 540, overflow: "hidden" }}>
        <View style={{ height: 42, flexDirection: "row", alignItems: "center", paddingHorizontal: 10 }}><Text class="text-primary text-sm" style={{ flex: 1 }}>Share {title(shareName())}</Text><Action icon="close" title="Close sharing" onPress={() => shareName.set("")} /></View>
        <WebView src={session.url(`/share/${shareName()}`)} style={{ flex: 1 }} />
      </View>
    </View>
  );

  const SignIn = () => (
    <View class="bg-window" style={{ flex: 1 }}>
      <View style={{ height: 58, alignItems: "center", justifyContent: "center" }}><Text class="text-primary" style={{ fontFamily: funnel, fontSize: 17, fontWeight: "600" }}>Finite.Computer</Text></View>
      <WebView src={session.loginUrl()} style={{ flex: 1 }} onError={(event) => console.error(`Sign-in: ${event.message}`)} onNavigationStateChange={(event) => {
        if (event.url.startsWith(`${session.host}/`) && !event.url.includes("/auth/") && !event.loading) load();
      }} />
    </View>
  );

  return (
    <View class="bg-sidebar text-primary" style={{ flex: 1 }}>
      <session.Bridge />
      {connecting() && !workspace() ? <View class="bg-window" style={{ flex: 1, alignItems: "center", justifyContent: "center", gap: 12 }}><Text class="text-primary" style={{ fontSize: 22, fontWeight: "600" }}>Finite.Computer</Text><Text class="text-secondary text-sm">Connecting…</Text></View> : null}
      {signedOut() && !fixture() ? <SignIn /> : null}
      {workspace() && !signedOut() ? <View style={{ flex: 1, flexDirection: "row" }}>{leftOpen() ? <Sidebar /> : null}<Chat />{rightOpen() && page() === "chat" ? <Desktop /> : null}</View> : null}
      {searchOpen() && workspace() ? <Search /> : null}
      {renameOpen() ? <Rename /> : null}
      {shareName() ? <Share /> : null}
    </View>
  );
}

mount(App);
