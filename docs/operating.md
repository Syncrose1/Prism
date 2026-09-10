# Operating Prism by hand

Prism is normally started once and then forgotten, which is the intent — but
"forgotten" and "unknown" are different things, and the second one is dangerous
when the operator is elsewhere. This is the whole surface.

## Is it running?

    systemctl --user status prismd          # state, uptime, last log lines
    systemctl --user is-active prismd       # just "active" or not
    ss -ltnp | grep prismd                  # WHICH ADDRESS it is serving on

That third one matters more than it looks. The bind is decided once, at
startup. If `prismd` starts before `tailscaled` has an address, it falls back to
`127.0.0.1` and serves only this machine — while still reporting itself
perfectly healthy. If the address shown is `127.0.0.1`, Prism is up and you
cannot reach it from anywhere else.

## Start, stop, restart

    systemctl --user restart prismd
    systemctl --user stop prismd
    systemctl --user start prismd

`Restart=always` and `StartLimitIntervalSec=0` mean it comes back from a crash
and is never rate-limited into giving up — so `stop` is the only thing that
actually keeps it down.

## Logs

    journalctl --user -u prismd -f                    # follow
    journalctl --user -u prismd --since "-1 hour"     # recent
    journalctl --user -u prismd -n 50 --no-pager      # last 50

A config error shows here and nowhere else: a bad `profile.toml` makes the
daemon exit at startup and restart every 5 seconds, forever, silently as far as
the browser is concerned.

## Reaching it

    http://<tailscale-ip>:9000/          the shell
    http://<tailscale-ip>:9000/rescue    the zero-JavaScript rescue page

`tailscale ip -4` gives the address. The rescue page is served unconditionally
at every tier, shares no code with the shell, and renders in a text browser over
SSH — it is what you use when the shell itself cannot load.

### From this machine

    prismd open            sign in and open a browser
    prismd open --print    print the URL instead

No phone. The reasoning is worth knowing, because "we skipped 2FA for
convenience" would be the wrong reason: the authenticator secret is a 0600 file
in the state directory, so **filesystem access as the owning user on this host
is already at least as strong as the second factor**. Anyone who can read
`console.key` can read `totp.secret` and mint codes forever. This takes a path
that was already open and makes it convenient rather than pretending it is shut.

It deliberately does *not* trust loopback. Loopback is not a place — SSH
forwards a port from anywhere, so a "local-only" route is silently a remote
route for anyone with SSH. Reading a 0600 file is a proof; arriving on 127.0.0.1
is not.

The key buys a **single-use grant, good for sixty seconds**, rather than going in
the URL itself. A URL is the leakiest thing on a computer — history, address bar
over a shared screen, `Referer` on the next click — and a long-lived credential
should never be one. What ends up in the bar has already stopped working.

### From anywhere else

Sign-in is the enrolled authenticator code, then the quick-unlock password.

    prismd enrol            # status (the secret is never displayed again)
    prismd enrol --reset    # revoke and replace, if the phone is lost
    prismd passwd           # change the quick-unlock password

`enrol --reset` is deliberately not an HTTP endpoint, not even a localhost one:
localhost is reachable through an SSH tunnel from anywhere, so a "local-only"
route would silently be a remote route.

## Facets

A facet is a workload Prism owns. Start and stop them from the shell, or:

    systemctl --user list-units 'prism-*'        # what is running under Prism
    systemctl --user stop prism-<id>.service     # stop one by hand

**A facet is only governed when Prism launched it.** Running the same command
from a terminal starts an ordinary process outside any Prism scope — no limits,
no attribution, invisible to the governor. If a workload matters, start it from
Prism.

The registry is `~/.config/prism/profile.toml`, hot-reloaded on change. Supported
keys per facet are `id`, `name`, `command`, `cwd`, `limits`, `enabled_if`,
`expose`, `pty` — note that `[facet.health]` appears in `architecture.md` but is
**not** implemented in the config schema, and including it will refuse the whole
profile at startup.

## When the machine is in trouble

    curl -s http://127.0.0.1:9000/api/health     # no auth required
    /rescue                                       # from any browser or w3m

Rescue can stop a facet and kill an attributed process without the shell, which
is the point of it existing as a separate artefact.
