# `gavin` (Global Action Version INspector)

Pipeline standardization tool.

If you have many repositories, it is often difficult to ensure that all the pipeline actions are using the same version.

`gavin` is a tool to inspect, validate, and update the versions of all pipeline actions globally.

With `gavin`, you can specify what version in your organization should be used as the standard version. `gavin` will then inspect all the actions in your organization and validate if they are using the standard version. If not, `gavin` will update the version of the action to the standard version.

## How it works

`gavin` will use git to perform a sparse checkout of all pipeline files in all the repositories in your organization to a temporary directory.

It uses `tokio` to perform all operations asynchronously in parallel to speed up the process.

The CLI is built with `clap` and the data is stored in a `sqlite` database. Configuration settings such as valid states are stored in a `gavinconfig.yml` file.

## Future work

- Add a `--dry-run` flag to inspect what `gavin` will do without actually performing any actions.
- Implement `--verbose` flag more to get more information about what `gavin` is doing.
- Expand support to more CI/CD platforms than just Azure DevOps such as GitHub Actions and GitLab CI/CD.
- Integrate with ticketing systems to create tickets for actions that are not using the standard version.
- Fix issue with repos that have spaces in the name.
- Read valid states from a config file so that users do not have to specify the valid states as flags.
- Fix bug: when checking gitversion task, inaccurately describes invalid states when there is no state definition.
- Fix bug: --check-task for gitversion shows incorrect text when there are no valid states defined for it. Does not fully implement TaskValidState throughout the code.
