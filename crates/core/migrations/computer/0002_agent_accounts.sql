-- Which of its owner's accounts at a provider each agent uses, where the
-- deployment lets a person connect several (its row's `accounts_max`): the
-- account's Pipes id, picked in settings. No row: the default, the oldest
-- connected (fragment_core::connection_accounts::chosen). A row whose
-- account is gone reads as none until it is next written.
CREATE TABLE agent_accounts (agent TEXT NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL, PRIMARY KEY (agent, provider));
