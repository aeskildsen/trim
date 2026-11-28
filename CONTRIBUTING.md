# Contributing

You can help with bug reports, feature requests, pull requests and documentation.

- Report bugs and request features in [Issues](../../issues).
- Send a pull request to fix a bug or to add a feature.
- Write each commit message as a [Conventional Commit](https://www.conventionalcommits.org/).
- Send improvements to the documentation as pull requests.
- Report security problems privately. See [SECURITY.md](SECURITY.md).
- When you contribute, you confirm that you have the right to license your contribution under the project [licence](LICENSE).

Before you send a pull request, run the checks in the Nix dev shell:

```sh
nix develop -c just test
nix develop -c just lint
```

## Contributions with AI help

You can use AI tools for code, documentation and GitHub communication. A person must examine and understand each contribution before it is sent. That person is responsible for the contribution.

- A contribution with AI help must meet the same project standards as other contributions.
- AI tools can help with reviews, but a person makes the final decision.
- Answer questions about your contribution yourself. Do not send messages between reviewers and AI tools.
- Do not put an AI identity in a `Signed-off-by` or `Co-authored-by` trailer. `Signed-off-by` is for a person who makes the stated certification.
- You do not have to tell about tools for grammar, spelling or clear language.
