import { Image, Text, View } from "nsh";
import { Shell } from "finite-computer/native";
import type { Fragment } from "./types";
import { labelOf } from "./types";

export function Icon(props: { name: string; size?: number; class?: string }) {
  const glyph: Record<string, string> = {
    plus: "+", chat: "✎", settings: "⚙", more: "•••", desktop: "▣", close: "×",
    reload: "↻", external: "↗", files: "▤", share: "⇧", search: "⌕", back: "‹",
    computer: "▰", people: "♧", archive: "▥", link: "⌁", check: "✓", grid: "⊞",
  };
  return <Text class={props.class} style={{ fontSize: props.size ?? 17, lineHeight: (props.size ?? 17) + 2, textAlign: "center" }}>{glyph[props.name] ?? "·"}</Text>;
}

export function Action(props: {
  icon?: string;
  label?: string;
  title: string;
  onPress: () => void;
  active?: boolean;
  disabled?: boolean;
  testID?: string;
  class?: string;
}) {
  return (
    <View
      testID={props.testID}
      accessibilityLabel={props.title}
      role="button"
      disabled={props.disabled}
      onPress={props.disabled ? undefined : props.onPress}
      class={`items-center justify-center rounded-lg ${props.active ? "bg-active" : ""} active:bg-hover ${props.class ?? ""}`}
      style={{ minHeight: 32, minWidth: props.icon && !props.label ? 32 : undefined, paddingHorizontal: props.label ? 10 : 4, flexDirection: "row", gap: 7, opacity: props.disabled ? 0.45 : 1 }}
    >
      {props.icon ? <Icon name={props.icon} class="text-icon" /> : null}
      {props.label ? <Text class="text-primary text-sm font-medium" numberOfLines={1}>{props.label}</Text> : null}
    </View>
  );
}

export function AgentAvatar(props: { identity?: string | null; picture?: string; size?: number }) {
  const size = props.size ?? 32;
  return (
    <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: Shell.agentColor(props.identity), alignItems: "center", justifyContent: "center", overflow: "hidden" }}>
      {props.picture ? <Image src={props.picture} style={{ width: size, height: size }} /> : <Text style={{ color: "#212121", fontSize: size * 0.52 }}>▰</Text>}
    </View>
  );
}

export function AppIcon(props: { name: string; size?: number }) {
  const size = props.size ?? 18;
  return (
    <View style={{ width: size, height: size, borderRadius: Math.max(3, size * 0.22), backgroundColor: Shell.appColor(labelOf(props.name)), alignItems: "center", justifyContent: "center" }}>
      <Text style={{ color: "white", fontSize: size * 0.56, lineHeight: size * 0.65 }}>⊞</Text>
    </View>
  );
}

export function FragmentMark(props: { fragment: Fragment; agents: Map<string, { fragment: string; identity: string }> }) {
  if (props.fragment.kind !== "chat") return <AppIcon name={props.fragment.name} />;
  const id = props.fragment.agents?.[0];
  const agent = id ? props.agents.get(id) : undefined;
  return <AgentAvatar identity={id} picture={props.fragment.picture ?? (agent ? props.fragment.picture : undefined)} size={28} />;
}

export function Section(props: { title: string; children?: any }) {
  return (
    <View class="w-full" style={{ marginTop: 48 }}>
      <Text class="text-primary" style={{ fontSize: 18, fontWeight: "600", marginBottom: 14 }}>{props.title}</Text>
      <View class="bg-muted border-border-strong rounded-2xl w-full" style={{ borderWidth: 1, paddingHorizontal: 20, paddingVertical: 8 }}>
        {props.children}
      </View>
    </View>
  );
}

export function SettingLine(props: { label: string; value?: string; children?: any; mono?: boolean }) {
  return (
    <View class="w-full border-border-strong" style={{ minHeight: 54, paddingVertical: 12, borderBottomWidth: 1, flexDirection: "row", alignItems: "center", gap: 16 }}>
      <Text class="text-secondary" style={{ width: 140, fontSize: 14 }}>{props.label}</Text>
      <View style={{ flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 10 }}>
        {props.value !== undefined ? <Text class={`text-primary ${props.mono ? "font-mono" : ""}`} style={{ fontSize: props.mono ? 12 : 14, flex: 1 }}>{props.value}</Text> : null}
        {props.children}
      </View>
    </View>
  );
}

export function Empty(props: { title: string; body?: string }) {
  return (
    <View class="flex-1 items-center justify-center" style={{ padding: 32, gap: 8 }}>
      <Text class="text-primary" style={{ fontSize: 18, fontWeight: "600" }}>{props.title}</Text>
      {props.body ? <Text class="text-secondary text-sm" style={{ textAlign: "center", maxWidth: 380 }}>{props.body}</Text> : null}
    </View>
  );
}
