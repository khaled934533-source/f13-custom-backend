````md
# f13-custom-backend

Railway-ready Rust backend scaffold for a Friday the 13th private-server project.

## Railway Environment Variables

```text
HOST=0.0.0.0
PORT=8080
DATABASE_URL=sqlite://f13.db
PUBLIC_URL=https://f13-custom-backend-production.up.railway.app
````

## Railway Settings

Root Directory:

```text
/
```

Build Command:

```text
```

Start Command:

```text
./bin/ccl-server
```

Railway provides HTTPS for the public domain and supplies the `PORT` environment variable.

## Health Endpoints

```text
GET /
GET /health
GET /api/v1/database/status
GET /api/v1/database_check
```

The database endpoints perform a real SQLite `SELECT 1` query instead of returning a hard-coded database status.

## API Scaffold

```text
POST /api/v1/login
POST /api/v1/auth/psn
GET  /api/v1/profiles/me
```

The login and profile responses are development placeholders. They are not a real PlayStation Network authentication system.

## Current Limitations

This project is currently a backend scaffold.

It does NOT yet implement:

* Real PSN authentication
* Friday the 13th matchmaking
* Lobby/session management
* Game-server protocol
* Client-side DNS redirection
* Game executable patching
* Complete original-game API compatibility

Deploying the Rust service to Railway alone does not make the original PlayStation game connect to it. The exact network protocol and game-side redirection mechanism still need to be implemented.

## Community

Discord:

https://discord.gg/SYaM9whT

## License

MIT

```
```
