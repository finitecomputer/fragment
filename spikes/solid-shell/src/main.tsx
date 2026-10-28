import {
  createMemo,
  createSignal,
  Errored,
  For,
  Loading,
  onCleanup,
} from "solid-js";
import { render } from "@solidjs/web";
import { api, displayName, isApp, type Fragment } from "./api";
import { Apps, Progress } from "./Apps";
import "./spike.css";

function Shell() {
  const [revision, setRevision] = createSignal(0);
  const fragments = createMemo(async () => {
    revision();
    return (await api<{ fragments: Fragment[] }>("GET", "/api/fragments"))
      .fragments;
  });
  const reload = () => setRevision((value) => value + 1);
  const socket = new WebSocket(
    `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/fragments/watch`,
  );
  socket.onmessage = reload;
  onCleanup(() => socket.close());
  function open(name: string) {
    window.open(
      `http://127.0.0.1:8790/auth/fragment?name=${encodeURIComponent(name)}`,
      "_blank",
      "noopener",
    );
  }
  return (
    <div class="solid-shell">
      <aside>
        <h2>Finite.Computer</h2>
        <small>Solid 2 · TypeScript spike</small>
        <Loading>
          <nav>
            <For
              each={fragments().filter(
                (fragment) => fragment.kind === "chat" && !fragment.archived,
              )}
            >
              {(fragment) => (
                <button onClick={() => open(fragment.name)}>
                  {displayName(fragment)}
                </button>
              )}
            </For>
          </nav>
        </Loading>
        <footer>
          <span>Apps</span>
          <a href="http://127.0.0.1:8790/settings">Settings</a>
        </footer>
      </aside>
      <main>
        <Errored fallback={(error) => <p role="alert">{String(error())}</p>}>
          <Loading fallback={<Progress text="Loading apps…" />}>
            <Apps
              fragments={fragments().filter(
                (fragment) => isApp(fragment) && !fragment.archived,
              )}
              reload={reload}
              open={open}
            />
          </Loading>
        </Errored>
      </main>
    </div>
  );
}
render(() => <Shell />, document.getElementById("root")!);
