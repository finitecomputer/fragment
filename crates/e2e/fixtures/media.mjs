// Deliveries and AI (slice F): pushes from a mutation and a job, and
// text (the model route), images (Workers AI) and videos (the model
// catalog) as a job's steps.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  notify_all({ title }, call) {
    call.push("*", { title, body: "from a mutation" });
    return { ok: true };
  }

  // a record on a channel a member may subscribe a URL to
  headline({ text }, call) {
    call.publish("news", { text });
    return { ok: true };
  }

  async announce({ who, title }, job) {
    return await job.push(who, { title, body: "from a job" });
  }

  async summarize({ text }, job) {
    return await job.ai.text({ model: "medium", prompt: text, reasoning_effort: "high" });
  }

  async decide(input, job) {
    return await job.ai.decide(input);
  }

  // an image inline, made in the job: `bytes` of a PNG's head and filler
  // (an operation's input is at most 256 KiB, so the job builds it)
  async decide_inline({ bytes }, job) {
    const data = "data:image/png;base64," + btoa("\x89PNG\r\n\x1a\n" + "x".repeat(bytes));
    return await job.ai.decide({ input: "an inline image", questions: { snack: { type: "predicate", instructions: "Is it a snack?" } }, media: [{ data }] });
  }

  // the high tier is off (decision 23): the step says so
  async summarize_high({ text }, job) {
    return await job.ai.text({ model: "high", prompt: text });
  }

  // whatever else the input names goes to the step too (steps, or a key it refuses)
  async draw({ prompt, path, ...rest }, job) {
    return await job.ai.image({ prompt, path, ...rest });
  }

  // meatproxy's shape: a prompt, a path and six seconds, the rest as the
  // input names it (a resolution, a ratio, or a key the step refuses)
  async film({ prompt, path, ...rest }, job) {
    return await job.ai.video({ prompt, path, duration: 6, ...rest });
  }
}
