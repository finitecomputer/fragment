The Fragment Way, as Paul sees it. These words are human-written and not to be revised or edited by an agent.

The idea of Fragments grew out of Finite's Sites and Brains, and the needs of the overall Finite.Computer product.

SITES SAYS

What if publishing and sharing a site privately or publicly was easier than making a Google Doc?

FRAGMENTS SAYS

What if that site had real state, and multiplayer? And collaboration built-in? In a way that agents natively understand (git) and the operations on that site visible to agents (mcp and cli).

BRAINS SAYS

What if your agent's llm-wiki (a folder of files following a convention) could be shared with other people to make an organizational brain?

FRAGMENT SAYS

What if that brain was a fragment as well?

FINITE COMPUTER SAYS

Users need a VM to run their agent, and frontend web ui to chat with it.

FRAGMENT SAYS

Bro, what if the computer's state and status was managed through a fragment. And the chat UI was a fragment too?

THE OVERALL THEME

It's not that I want to convert everything into a Fragment, it's that Fragment turns out to be a really useful primitive, with surpisingly few special cases, for what we want to do.

We win by keeping Fragments as the organizing principle and fighting the urge to make Fragments bloated, overly complicated, overly opinionated over the underlying Cloudflare features they proxy. Everything that cleanly fits into the Fragments paradigm becomes a feature our users can use for all of the Apps they build, directly on our platform or through other agents they onboard.

We should feel relatively fearless when we add features that "do more with the abilities and design constraints fragments already have". We should slow down and discuss architecture when something adds complexity or extra special casing to the Fragments themselves. It's much better to put something in the "special cases inventory" than to ship something to all Fragments that only some Fragments need.

The trajectory of the repo should be to trim down the complexity, code size, indirection, multiple sources of truth, and layering of Fragments. Fragments should eventually be more "pay for what you use" than they are right now. All without lowering the expressivity or security / ownership story.

Also, over time the repo should minimize docs and comments and focus on the code being the source of truth. To do this, the code must stay small and minimally complected.
