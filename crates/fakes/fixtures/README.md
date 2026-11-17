# Fixtures

- `old-home/`: an old agent's home, synthetic and already filtered (no
  secret, cache or runtime file), laid out as finite.computer kept one
  (`/data`: `agent/hermes-home/`, `workspace/`, `sites/`). What `fragment
  home` and the `import-agent-home` skill are tested on (FIN-463). Its
  Hermes `state.db` is made by the tests that read it.
- `old-home.layout.json`: where that home keeps things, as a platform's
  import skill carries its layout.
