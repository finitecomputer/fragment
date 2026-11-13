//! The identity door: a self-hosted box's sign-in, behind the WorkOS
//! endpoints the platform already calls (docs/self-host.md, seam 4). The
//! cell signs people in through it by configuration alone (`WORKOS_API_URL`
//! the door's public URL, `WORKOS_CLIENT` and `WORKOS_KEY` one of its
//! clients), as celld reads the Secrets Store's bindings from OpenBao: the
//! cell has no code of its own for it. Behind those endpoints the door is
//! an OpenID Connect relying party of whichever provider its config names:
//! on a tailnet, Tailscale's tsidp (which asks tailscaled who the device
//! is, so the door does not); on an intranet, the org's own IdP. Identity
//! is the provider's, never the network's.
//!
//! **Its surface**, WorkOS's as the cell uses it, and the provider's callback:
//!
//! - `GET /user_management/authorize?client_id&redirect_uri&response_type=code&state`:
//!   the client and its registered callback checked, then the browser sent
//!   to the provider (`oidc::authorize_url`: the code flow, PKCE's S256,
//!   a nonce, the door's sealed state).
//! - `GET /oidc/callback?code&state`: the provider's answer. The code
//!   redeemed, the id_token's signature checked by the provider's JWKS
//!   (`jws`) and its claims (`oidc::verified`), the person read
//!   (`person`), then back to the platform's callback with a one-time code
//!   of the door's (`codes`), or an OAuth error the platform's page shows.
//! - `POST /user_management/authenticate {client_id, client_secret,
//!   grant_type, code}`: the door's code, with its client's secret, for
//!   `{user: {id, email, email_verified: true}}`, once. No access token:
//!   the door keeps no session, so the platform's sign-out ends its own
//!   alone (as fragment#1244 decided: tsidp has no end-session endpoint).
//! - `GET /user_management/sessions/logout?return_to`: back to a client's
//!   own origin (only a session signed in elsewhere before names one).
//! - Everything else, WorkOS Pipes among it: 404.
//!
//! **Why it is safe:**
//!
//! 1. *The provider is the one configured.* Its metadata's `issuer` must
//!    be exactly the configured one; its endpoints https (http only beside a
//!    loopback issuer: dev). An id_token is signed RS256 or ES256 by a key
//!    its JWKS publishes (never `none`, an HMAC, or a key the token
//!    carries), for this client, this sign-in's nonce, within its times.
//! 2. *Only the door can redeem a code it began.* The PKCE verifier and the
//!    nonce are derived from the door's secret and its state, which is
//!    sealed with an HMAC under that secret: nothing is stored, a restart
//!    in the middle finishes, and no one else can make the verifier.
//! 3. *The platform binds the round trip to one browser.* The platform's
//!    own state rides inside the door's and is handed back to its callback,
//!    where only the browser holding its cookie finishes the sign-in.
//! 4. *The email is never guessed.* It must be a mailbox, and either the
//!    provider verified it or its domain is one the operator vouches for
//!    (tsidp sends no `email_verified`; a GitHub login's `user@github` is no
//!    mailbox, so it is refused whatever is vouched).
//! 5. *A person is the provider's subject.* `(issuer, sub)` is hashed into
//!    the subject the platform keys them by, so another provider's `sub` is
//!    never theirs.
//! 6. *The door's codes are bound, single-use, short-lived, in memory.* 32
//!    random bytes, good for 120 s, once, for the client they were issued
//!    to, exchanged only with that client's secret (its SHA-256, compared
//!    in constant time). A restart forgets them: the one sign-in caught
//!    between the provider's callback and the platform's exchange (a
//!    second's window) is told to start again; none is replayed after.
//! 7. *Callbacks are registered.* The door sends a browser only to a
//!    callback its client registered, exactly; before the client and
//!    callback are known, or when the state does not open, a refusal is a
//!    page, never a redirect.

mod codes;
pub mod config;
pub mod jws;
pub mod oidc;
pub mod person;
mod provider;
mod serve;
pub mod state;

pub use codes::{CODES_MAX, CODE_TTL};
pub use config::{Client, Config, ConfigError, ProviderConfig, CALLBACK_PATH};
pub use person::{Person, Vouched};
pub use serve::{Door, StartError, BODY_BYTES_MAX, CONNECTIONS_MAX};
