# KLAY Website

Static Arabic RTL landing page for the KLAY Friday the 13th community project.

## Preview locally

Open `index.html` in a browser, or run a local static server from the repository root:

```powershell
py -m http.server 8000 --directory website
```

Then visit `http://localhost:8000`.

## Epic sign-in status

The website button now redirects to `/auth/epic/login`. The Rust backend on this branch still needs that route and its callback before sign-in can work. Do not deploy this as a working login yet.

A real Epic Account Services web login requires:
- An HTTPS callback URL registered for the correct Epic client.
- A server-side authorization-code exchange.
- One-time OAuth `state` validation to protect against CSRF.
- Epic Client ID, Client Secret, and Deployment ID stored as server environment variables.
- Secure KLAY session handling after Epic identity is verified.

Never put the Epic Client Secret in `app.js` or any other browser-side file.

Official Epic documentation:
https://dev.epicgames.com/docs/epic-online-services/accounts-and-social/eos-epic-account-services/getting-started

This website is a community project and is not officially affiliated with IllFonic, Gun Media, or Epic Games.
