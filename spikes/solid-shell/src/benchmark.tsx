// Synthetic, rendering-only fixture. The production Apps component runs
// against controlled local responses; no benchmark data enters the product.
import { createSignal, flush, Errored } from "solid-js";
import { render } from "@solidjs/web";
import { Apps } from "./Apps";
import type { Fragment } from "./api";
const [rows, setRows] = createSignal<Fragment[]>([]);
render(
  () => (
    <Errored fallback={(error) => <p>{String(error())}</p>}>
      <Apps fragments={rows()} reload={() => {}} open={() => {}} />
    </Errored>
  ),
  document.getElementById("root")!,
);
declare global {
  interface Window {
    solidBenchmark: { update: (rows: Fragment[]) => void };
  }
}
window.solidBenchmark = {
  update(next) {
    setRows(next);
    flush();
  },
};
