## Description
<!-- Provide a clear and concise description of the changes in this PR. -->

## Type of Change
<!-- Check all that apply -->
- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactoring (no functional changes)
- [ ] Performance improvement
- [ ] Test improvements
- [ ] CI/CD changes
- [ ] Dependency updates

## Related Issues
<!-- Link any related issues using "Fixes #123" or "Relates to #123" -->
Fixes #

## Testing
<!-- Describe the tests you ran and how to verify the changes -->
- [ ] All existing tests pass (`bash ci/scripts/test-all.sh`)
- [ ] New tests added for new functionality
- [ ] Manual testing performed (describe below)

### Manual Testing Steps
1.
2.
3.

## Checklist
<!-- Ensure all items are checked before requesting review -->
- [ ] Code follows the project's style guidelines (`cargo fmt --all -- --check`)
- [ ] Code compiles without warnings (`RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features`)
- [ ] Clippy passes with no warnings (`cargo clippy --workspace --all-targets --all-features -- -D warnings`)
- [ ] All tests pass (`cargo test --workspace --all-targets --all-features`)
- [ ] Doc tests pass (`cargo test --workspace --doc --all-features`)
- [ ] Coverage meets 80% threshold (`cargo llvm-cov --workspace --all-features --all-targets --fail-under-lines 80`)
- [ ] No backend types leaked into interface crates (`bash ci/scripts/test-interface-no-backend-leakage.sh`)
- [ ] Real adapter dependency check passes (`bash ci/scripts/test-real-adapter-deps.sh`)
- [ ] Coverage map validation passes (`bash ci/scripts/test-coverage-map.sh`)
- [ ] Commit messages follow conventional commits format
- [ ] Documentation updated (if applicable)
- [ ] CHANGELOG updated (if applicable)

## Screenshots / Recordings
<!-- If applicable, add screenshots or recordings demonstrating the changes -->

## Additional Notes
<!-- Any additional information, configuration, or context that reviewers should know -->