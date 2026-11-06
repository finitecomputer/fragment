import type { DesignSystem, Page, SlideMeta } from '@open-slide/core';
import type { ReactNode } from 'react';

export const design: DesignSystem = {
  palette: { bg: '#f7f5f2', text: '#1c1b1a', accent: '#2f6fde' },
  fonts: {
    display: '"Geist Variable", Geist, -apple-system, BlinkMacSystemFont, system-ui, sans-serif',
    body: '"Geist Variable", Geist, -apple-system, BlinkMacSystemFont, system-ui, sans-serif',
  },
  typeScale: { hero: 168, body: 40 },
  radius: 16,
};

const ink = { text: '#1c1b1a', soft: '#5d5a56', rule: '#e2ddd6', accent: '#2f6fde' };

function Frame({ kicker, children }: { kicker: string; children: ReactNode }) {
  return (
    <div
      style={{
        width: '100%',
        height: '100%',
        background: 'var(--osd-bg)',
        color: ink.text,
        fontFamily: 'var(--osd-font-body)',
        padding: 140,
        boxSizing: 'border-box',
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      <div style={{ fontSize: 28, letterSpacing: 2, textTransform: 'uppercase', color: ink.accent }}>{kicker}</div>
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', justifyContent: 'center' }}>{children}</div>
    </div>
  );
}

function Point({ title, body }: { title: string; body: string }) {
  return (
    <Frame kicker={title}>
      <div style={{ fontFamily: 'var(--osd-font-display)', fontSize: 96, lineHeight: 1.15, maxWidth: 1500 }}>{body}</div>
    </Frame>
  );
}

const Cover: Page = () => (
  <Frame kicker="Slides">
    <div style={{ fontFamily: 'var(--osd-font-display)', fontSize: 168, lineHeight: 1.05, fontWeight: 600 }}>Your deck</div>
    <div style={{ marginTop: 48, fontSize: 44, color: ink.soft, maxWidth: 1300, lineHeight: 1.5 }}>
      Present it live, and everyone watching follows along.
    </div>
  </Frame>
);

const Present: Page = () => <Point title="Present live" body="Press Present live. Everyone with the link moves with you, page by page." />;

const Notes: Page = () => <Point title="Notes" body="Leave a note on any page. Your agent reads them and makes the change." />;

const Edit: Page = () => (
  <Frame kicker="Make it yours">
    <div style={{ fontFamily: 'var(--osd-font-display)', fontSize: 96, lineHeight: 1.15, maxWidth: 1500 }}>
      Ask your agent to write the deck you need.
    </div>
    <div style={{ marginTop: 56, paddingTop: 40, borderTop: `2px solid ${ink.rule}`, fontSize: 36, color: ink.soft, lineHeight: 1.6 }}>
      Each page is a React component in slides/deck/index.tsx.
    </div>
  </Frame>
);

export const meta: SlideMeta = {
  title: 'Your deck',
  createdAt: '2026-11-06T14:15:36.241Z',
};

export const notes: (string | undefined)[] = [
  'Welcome everyone, and share the link so they can follow.',
  'Anyone with the link follows the presenter. They can stop following at any time.',
  'Notes stay on their page until someone resolves them.',
  'Ask for changes in plain words.',
];

export default [Cover, Present, Notes, Edit] satisfies Page[];
