# SignPath configuration

Release builds are signed through [SignPath Foundation](https://signpath.org).
These are the artifact configurations the SignPath project uses; paste each
into the SignPath project with the slug matching its file name.

| Slug | File | Signs |
| --- | --- | --- |
| `exe` | `artifact-configuration/exe.xml` | `afterm.exe`, before it goes into the installers |
| `installers` | `artifact-configuration/installers.xml` | the MSI and the NSIS setup program |

The workflow (`.github/workflows/build.yml`) signs when these repository
settings exist (Settings ▸ Secrets and variables ▸ Actions):

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `SIGNPATH_ORGANIZATION_ID` | the organization id from SignPath |
| Variable | `SIGNPATH_PROJECT_SLUG` | the SignPath project slug, e.g. `afterm` |
| Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | `test-signing` while setting up, then `release-signing` |
| Secret | `SIGNPATH_API_TOKEN` | an API token for a SignPath user with submitter rights |

Without `SIGNPATH_ORGANIZATION_ID` the workflow builds unsigned, as before.
With it, every release is signed, and a manual run is signed when *Sign with
SignPath* is ticked. Each signed build sends two signing requests, one for the
executable and one for the installers; under the release-signing policy each
waits in SignPath for an approver to approve it.
