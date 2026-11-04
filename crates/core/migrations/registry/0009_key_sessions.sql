ALTER TABLE sessions ADD COLUMN device_key TEXT;
CREATE UNIQUE INDEX sessions_device_key ON sessions (device_key) WHERE device_key IS NOT NULL;
