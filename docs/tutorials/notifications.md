# Set up notifications

Optional. Two independent parts; set up either or both:

| Part                       | Tells you                                                            | Needs                   |
| -------------------------- | -------------------------------------------------------------------- | ----------------------- |
| **Email** (`SMTP_*`)       | The day's commits were pushed (SHAs, log lines, links); a day failed; the service was stopped. | An SMTP account (Gmail works) |
| **Healthcheck** (`HEALTHCHECK_URL`) | The machine is off, lost power, or the service is down/crashing. | A free [healthchecks.io](https://healthchecks.io) account |

Why two parts: a machine that loses power, or a process killed with `SIGKILL`, can't send anything.
Instead the service pings healthchecks.io every few minutes, and healthchecks.io alerts you
when the pings stop.

What you receive, at most:

| Event                                   | Email                                                  |
| --------------------------------------- | ------------------------------------------------------ |
| Daily run pushed its commits            | `[auto-git-commit] 2026-10-03: 3 commits pushed`        |
| Run failed / push failed                | One per day, e.g. `… 2026-10-03: daily run failed` (not one per 30-minute retry) |
| …and later recovered                    | `… 2026-10-03: pending commits pushed`                  |
| Service stopped (`systemctl stop`, `docker stop`, reboot, shutdown) | `… 2026-10-03: service stopped on <host>` |
| No pings for longer than the grace time | From healthchecks.io: check is **down** (and **up** again when it's back) |

A quick reboot sends only the "service stopped" email: the service is back before the
healthcheck's grace time runs out.

## 1. Email with Gmail

Gmail needs an **app password** (your normal password doesn't work over SMTP):

1. Turn on 2-Step Verification for your Google account, if it isn't already.
2. Open <https://myaccount.google.com/apppasswords>, create an app password named e.g.
   `auto-git-commit`, and copy the 16-character password. Google shows it in groups
   (`abcd efgh ijkl mnop`): **remove the spaces**. Env files take values unquoted, and a
   space breaks `set -a; . ./.env` (the shell runs the rest as a command).
3. Add to your env file (`/etc/auto-git-commit-tool/env` for the systemd install, `.env`
   otherwise):

   ```dotenv
   SMTP_HOST=smtp.gmail.com
   SMTP_USERNAME=you@gmail.com
   SMTP_PASSWORD=abcdabcdabcdabcd
   ```

   Emails go from and to `you@gmail.com` by default. Set `NOTIFY_EMAIL_TO` to send them
   elsewhere. Other providers work the same way: port `465` (default) uses implicit TLS, any
   other port (e.g. `SMTP_PORT=587`) uses STARTTLS. Unencrypted SMTP is not supported.

### Keep them out of the way, but searchable

Every subject starts with `[auto-git-commit] <YYYY-MM-DD>:`. In Gmail, create a filter
(search box → *Show search options*):

- **Subject:** `auto-git-commit pushed` → *Create filter* → **Skip the Inbox**, **Apply the
  label** `auto-git-commit`.

Successful days are then filed silently under the label, while failures and "service stopped"
still land in the inbox. Drop the word `pushed` to file everything. Searching later:

```text
label:auto-git-commit 2026-10-03
subject:auto-git-commit "service stopped"
```

## 2. Healthcheck with healthchecks.io

1. Sign up at <https://healthchecks.io> (the free plan is enough) and **Add Check**.
2. Name it, e.g. `auto-git-commit <host>`, and set the schedule:
   - **Period:** 5 minutes (match `HEALTHCHECK_INTERVAL_MINUTES`, default `5`)
   - **Grace time:** 15 minutes (leave room for a reboot and a slow network at boot)
3. Copy the check's ping URL (`https://hc-ping.com/<uuid>`) and add it to the env file:

   ```dotenv
   HEALTHCHECK_URL=https://hc-ping.com/your-uuid
   ```

   Treat it like a password: whoever has it can report the service as alive.
4. Under **Integrations**, email to your account is on by default. You can add others
   (Telegram, ntfy, Slack…) there; the service doesn't need to change.

You'll get a **down** alert about 20 minutes (period + grace) after the last ping. Pings start
only after the service has started successfully, so a service stuck failing at startup (bad
token, no network for 25 minutes) is reported as down too.

## 3. Apply and test

```bash
sudo systemctl restart auto-git-commit-tool    # systemd install: pick up the new env file
```

Send a test email and one ping:

```bash
# systemd + Docker
sudo docker run --rm --env-file /etc/auto-git-commit-tool/env auto-git-commit-tool notify-test
# systemd + native
sudo sh -c 'set -a; . /etc/auto-git-commit-tool/env; auto-git-commit-tool notify-test'
# local, with a .env file
set -a; . ./.env; set +a; auto-git-commit-tool notify-test
```

`check` also logs in to the SMTP server (without sending anything). healthchecks.io should show
the check as **up** within a minute of the restart.

## Turn notifications off

- Everything, keeping the settings: `NOTIFY_ENABLED=false`, then restart the service.
- Only email: remove `SMTP_HOST` (and the other `SMTP_*`/`NOTIFY_EMAIL_*` settings).
- Only the healthcheck: remove `HEALTHCHECK_URL`.

When you stop the pings, also **pause** (or delete) the check on healthchecks.io, otherwise it
reports the service as down.
