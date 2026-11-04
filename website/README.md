# KLAY Website

Static Arabic RTL landing page for the KLAY Friday the 13th community project.

## Preview locally

Open `index.html` in a browser, or run a local static server from the repository root:

```powershell
py -m http.server 8000 --directory website
```

Then visit `http://localhost:8000`.

## Epic sign-in configuration

The Rust backend implements these same-origin routes:

- `GET /auth/epic/login` — begins Epic Account Services authorization.
- `GET /auth/epic/callback` — validates the one-time browser-bound OAuth state, exchanges the authorization code on the server, and creates a KLAY session.
- `GET /auth/epic/me` — returns the signed-in Epic-backed KLAY profile using the HttpOnly session cookie.

Configure these environment variables on the backend host:

- `EPIC_CLIENT_ID` — OAuth client ID from the Epic Developer Portal.
- `EPIC_CLIENT_SECRET` — OAuth client secret; server-side only, never commit or expose it.
- `EPIC_DEPLOYMENT_ID` — deployment ID for the selected Epic sandbox.
- `EPIC_REDIRECT_URI` — exact HTTPS callback URL, for example `https://YOUR-DOMAIN/auth/epic/callback`.

Register the exact value of `EPIC_REDIRECT_URI` in the Epic client Redirect URLs. The callback must be served over HTTPS. The backend requires all four variables; the client secret is read only by the server. OAuth state is random, stored for ten minutes, checked against an HttpOnly/Secure/SameSite cookie, and consumed once. The KLAY session is also issued in an HttpOnly/Secure/SameSite cookie.

Epic OAuth proves an Epic Account Services identity only. It does not authenticate a PlayStation Network account, grant PSN entitlements, or make the original PS4/PS5 game protocol compatible. The existing `/api/v1/auth/psn` endpoint remains explicitly unimplemented.

See the [Epic Auth Web APIs documentation](https://dev.epicgames.com/docs/web-api-ref/authentication) for the authorization-code and token-exchange contract. Never place `EPIC_CLIENT_SECRET` in browser JavaScript.

This website is a community project and is not officially affiliated with IllFonic, Gun Media, or Epic Games.
