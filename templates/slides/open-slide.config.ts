import type { OpenSlideConfig } from '@open-slide/core';

// One deck per fragment: no list of decks, and its page opens the deck.
const config: OpenSlideConfig = {
  build: { showSlideBrowser: false },
};

export default config;
