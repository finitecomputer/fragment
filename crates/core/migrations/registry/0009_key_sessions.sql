-- A person's approved key (an iPhone's, say) signs in to fragments' web
-- pages through one platform session of its own, never a bearer one: its
-- token is never kept, so nothing can present it. Each site session
-- minted from it lives only while that key is still the person's
-- (signin.rs `live_session`, `redeem`). One such session a key.
ALTER TABLE sessions ADD COLUMN device_key TEXT;
CREATE UNIQUE INDEX sessions_device_key ON sessions (device_key) WHERE device_key IS NOT NULL;
