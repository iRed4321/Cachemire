# Security policy

Cachemire handles credentials: Redis passwords, SSH keys and passwords, AWS sign-ins. If you find a way they could leak or be misused, please report it privately rather than in a public issue.

## Reporting a vulnerability

Use **[Report a vulnerability](https://github.com/iRed4321/Cachemire/security/advisories/new)** on the repository's Security tab. Only you and the maintainer can see the report.

If you can't use GitHub, write to leoparentperso@gmail.com.

Please include the version (shown by the **i** button of the title bar), your OS, and the steps to reproduce. Cachemire is maintained by one person on their free time: expect a first answer within a week. A fix is released as soon as it's ready, and you'll be credited in the advisory unless you'd rather not be.

## Supported versions

Fixes go into the next release, stable and beta alike. Older versions don't get fixes: the app offers to update itself (Windows) or links to the new release (Linux).

## In scope

- Passwords, keys or tokens written in plain text, logged, or sent anywhere other than the server they belong to
- The SSH tunnel accepting a host key it shouldn't, or the TLS connection skipping verification
- The updater fetching or running something other than an official release
- A Redis server, or data read from it, getting the app to run code or touch files

Bugs that need someone already in control of your user account or machine are out of scope.
