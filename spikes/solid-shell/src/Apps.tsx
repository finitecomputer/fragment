import {
  createMemo,
  createSignal,
  For,
  Loading,
  onCleanup,
  Show,
} from "solid-js";
import {
  api,
  canEdit,
  displayName,
  pathOf,
  rename,
  type Address,
  type Candidate,
  type Fragment,
  type Status,
} from "./api";

export function Progress(props: { text: string }) {
  const [seconds, setSeconds] = createSignal(0);
  const started = Date.now();
  const timer = setInterval(
    () => setSeconds(Math.floor((Date.now() - started) / 1000)),
    1000,
  );
  onCleanup(() => clearInterval(timer));
  return (
    <span class="activity">
      <span class="activity-spinner" aria-hidden="true" />
      <span role="status">{props.text}</span>
      <span aria-hidden="true">{seconds()}s</span>
    </span>
  );
}

function IconEditor(props: { fragment: Fragment; reload: () => void }) {
  const [hint, setHint] = createSignal("");
  const [candidates, setCandidates] = createSignal<Candidate[]>([]);
  const [busy, setBusy] = createSignal("");
  const [error, setError] = createSignal("");
  const path = () => `${pathOf(props.fragment.name)}/icon`;
  async function make(event: SubmitEvent) {
    event.preventDefault();
    setBusy("Making icons…");
    setError("");
    try {
      const out = await api<{ candidates: Candidate[] }>(
        "POST",
        `${path()}/make`,
        { hint: hint() || undefined },
      );
      setCandidates(out.candidates);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy("");
    }
  }
  async function put(body: Blob | Uint8Array) {
    setBusy("Saving icon…");
    setError("");
    try {
      const response = await fetch(path(), {
        method: "PUT",
        headers: { "x-fragment-shell": "1" },
        body: body as BodyInit,
      });
      if (!response.ok) throw new Error((await response.json()).message);
      props.reload();
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy("");
    }
  }
  return (
    <section class="icon-editor">
      <h2>App icon</h2>
      <form class="icon-make" onSubmit={make}>
        <input
          aria-label="Icon description"
          placeholder="Describe it (optional)"
          maxlength={200}
          value={hint()}
          onInput={(event) => setHint(event.currentTarget.value)}
        />
        <button disabled={!!busy()}>Make icons</button>
      </form>
      <Show when={busy()}>{(text) => <Progress text={text()} />}</Show>
      <div class="icon-picks">
        <For each={candidates()}>
          {(candidate) => (
            <button
              class="icon-pick"
              disabled={!!busy()}
              onClick={() =>
                put(
                  Uint8Array.from(atob(candidate.png), (character) =>
                    character.charCodeAt(0),
                  ),
                )
              }
            >
              <img
                alt="Use this icon"
                src={`data:image/png;base64,${candidate.png}`}
              />
            </button>
          )}
        </For>
      </div>
      <label class="solid-upload">
        Upload
        <input
          type="file"
          accept="image/*"
          disabled={!!busy()}
          onChange={(event) => {
            const file = event.currentTarget.files?.[0];
            if (file) void put(file);
          }}
        />
      </label>
      <p role="alert">{error()}</p>
    </section>
  );
}

function PublicAddress(props: { fragment: Fragment }) {
  const [revision, setRevision] = createSignal(0);
  const name = createMemo(() => props.fragment.name);
  const address = createMemo(async () => {
    revision();
    return api<Address>("GET", `${pathOf(name())}/address`);
  });
  const [label, setLabel] = createSignal(
    displayName(props.fragment)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, ""),
  );
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  async function claim(event: SubmitEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api<Address>("PUT", `${pathOf(props.fragment.name)}/address`, {
        label: label(),
      });
      setRevision((value) => value + 1);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Loading fallback={<Progress text="Loading address…" />}>
      <section>
        <h2>Public address</h2>
        <Show
          when={address().address}
          fallback={
            <>
              <form class="app-address-form" onSubmit={claim}>
                <label class="app-address-field">
                  <input
                    aria-label="Public address"
                    required
                    maxlength={address().maxLength}
                    pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                    value={label()}
                    disabled={busy()}
                    onInput={(event) => setLabel(event.currentTarget.value)}
                  />
                  <span>{address().suffix}</span>
                </label>
                <button disabled={busy()}>Claim address</button>
              </form>
              <p class="muted app-address-note">
                First come, first served. This address stays with this app.
              </p>
            </>
          }
        >
          {(kept) => (
            <a target="_blank" rel="noopener" href={kept().url}>
              {new URL(kept().url).host}
            </a>
          )}
        </Show>
        <Show when={busy()}>
          <Progress text="Claiming address…" />
        </Show>
        <p role="alert">{error()}</p>
      </section>
    </Loading>
  );
}

export function AppRow(props: {
  fragment: Fragment;
  reload: () => void;
  open: (name: string) => void;
}) {
  const f = () => props.fragment;
  const name = createMemo(() => f().name);
  const [editing, setEditing] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const [busy, setBusy] = createSignal("");
  const [error, setError] = createSignal("");
  const [panel, setPanel] = createSignal("");
  const [card, setCard] = createSignal<string>();
  const [statusRevision, setStatusRevision] = createSignal(0);
  const status = createMemo(async () => {
    statusRevision();
    return api<Status>("GET", `${pathOf(name())}/status`);
  });
  const address = createMemo(async () => {
    statusRevision();
    return api<Address>("GET", `${pathOf(name())}/address`);
  });
  const addressUrl = () => address().address?.url ?? status().urls.canonical;
  onCleanup(() => {
    const url = card();
    if (url) URL.revokeObjectURL(url);
  });
  async function run(label: string, operation: () => Promise<unknown>) {
    if (busy()) return;
    setBusy(label);
    setError("");
    try {
      await operation();
      props.reload();
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy("");
    }
  }
  async function newPreview() {
    await run("Taking a new preview…", async () => {
      const ask = await api<{ since: number }>(
        "POST",
        `${pathOf(f().name)}/card`,
      );
      const end = Date.now() + 180000;
      while (Date.now() < end) {
        const current = await api<Status>("GET", `${pathOf(f().name)}/status`);
        if (current.page && current.page.at >= ask.since) {
          const response = await fetch(`${pathOf(f().name)}/card`, {
            headers: { "x-fragment-shell": "1" },
          });
          if (!response.ok) throw new Error("The preview did not load");
          const previous = card();
          if (previous) URL.revokeObjectURL(previous);
          setCard(URL.createObjectURL(await response.blob()));
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
      throw new Error("The preview is still being taken");
    });
  }
  const toggle = (name: string) =>
    setPanel((value) => (value === name ? "" : name));
  return (
    <article class="app-row" data-fragment={f().name}>
      <div class="app-row-head">
        <button
          class="app-row-art"
          aria-label={`Open ${displayName(f())}`}
          onClick={() => props.open(f().name)}
        >
          <Show
            when={f().picture}
            fallback={<span class="solid-glyph">▢</span>}
          >
            {(picture) => <img alt="" src={picture()} />}
          </Show>
        </button>
        <div class="app-row-text">
          <Show
            when={editing()}
            fallback={
              <div class="app-title-line">
                <button
                  class="app-row-title"
                  onClick={() => props.open(f().name)}
                >
                  {displayName(f())}
                </button>
                <Show when={canEdit(f())}>
                  <button
                    class="app-title-edit"
                    aria-label={`Rename ${displayName(f())}`}
                    onClick={() => {
                      setDraft(displayName(f()));
                      setEditing(true);
                    }}
                  >
                    ✎
                  </button>
                </Show>
              </div>
            }
          >
            <form
              class="app-name-form"
              onSubmit={(event) => {
                event.preventDefault();
                void run("Saving name…", async () => {
                  await rename(f(), draft().trim());
                  setEditing(false);
                });
              }}
            >
              <input
                class="app-name-input"
                aria-label="App name"
                required
                maxlength={120}
                value={draft()}
                onInput={(event) => setDraft(event.currentTarget.value)}
              />
              <button disabled={!!busy()}>Save</button>
              <button type="button" onClick={() => setEditing(false)}>
                Cancel
              </button>
            </form>
          </Show>
          <Loading fallback={<span>Loading address…</span>}>
            <a
              class="app-row-url"
              href={addressUrl()}
              target="_blank"
              rel="noopener"
            >
              {new URL(addressUrl()).host} ↗
            </a>
          </Loading>
          <span class="app-row-access">
            {f().sharing?.visibility === "public"
              ? "Public"
              : f().sharing?.visibility === "link"
                ? "Anyone with the link"
                : "Restricted"}
          </span>
        </div>
        <button
          class="app-dock"
          role="switch"
          aria-label={`Show ${displayName(f())} in dock`}
          aria-checked={f().dock !== false ? "true" : "false"}
          disabled={!!busy()}
          onClick={() =>
            run("Saving shortcut…", () =>
              api("PUT", `/api/fragments/${f().name}/dock`, {
                dock: f().dock === false,
              }),
            )
          }
        >
          <span class="app-dock-track" />
          Dock
        </button>
      </div>
      <div class="app-row-tools">
        <Show when={canEdit(f())}>
          <button
            class="quiet"
            aria-expanded={panel() === "appearance" ? "true" : "false"}
            onClick={() => toggle("appearance")}
          >
            Appearance
          </button>
        </Show>
        <button
          class="quiet"
          aria-expanded={panel() === "sharing" ? "true" : "false"}
          onClick={() => toggle("sharing")}
        >
          Sharing
        </button>
        <Show when={f().role === "owner"}>
          <button
            class="quiet"
            aria-expanded={panel() === "address" ? "true" : "false"}
            onClick={() => toggle("address")}
          >
            Address
          </button>
        </Show>
      </div>
      <Show when={panel() === "appearance"}>
        <div class="app-row-panel">
          <IconEditor fragment={f()} reload={props.reload} />
          <Show when={f().sharing?.visibility !== "members"}>
            <section class="app-preview-editor">
              <h2>Preview</h2>
              <Show when={card()}>
                {(url) => <img class="app-preview-image" alt="" src={url()} />}
              </Show>
              <button class="quiet" disabled={!!busy()} onClick={newPreview}>
                New preview
              </button>
            </section>
          </Show>
        </div>
      </Show>
      <Show when={panel() === "sharing"}>
        <div class="app-row-panel" data-share-editor={f().name}>
          <iframe
            class="app-sharing-frame"
            title={`Sharing ${displayName(f())}`}
            src={`/share/${f().name}`}
            allow="clipboard-write"
          />
        </div>
      </Show>
      <Show when={panel() === "address"}>
        <div class="app-row-panel" data-address-editor={f().name}>
          <PublicAddress fragment={f()} />
        </div>
      </Show>
      <Show when={busy()}>{(text) => <Progress text={text()} />}</Show>
      <p role="alert">{error()}</p>
    </article>
  );
}

export function Apps(props: {
  fragments: Fragment[];
  reload: () => void;
  open: (name: string) => void;
}) {
  return (
    <div class="apps-layout">
      <header class="apps-intro">
        <h1>Apps</h1>
      </header>
      <div class="apps-list">
        <For each={props.fragments} keyed={(fragment) => fragment.name}>
          {(fragment) => (
            <AppRow
              fragment={fragment()}
              reload={props.reload}
              open={props.open}
            />
          )}
        </For>
      </div>
    </div>
  );
}
