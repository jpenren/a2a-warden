# Pull Request Template

## Description

<!-- Describe the changes in this PR. What problem does it solve? -->

## Type of Change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactoring (no functional changes)
- [ ] Performance improvement
- [ ] Test improvements
- [ ] CI/CD changes
- [ ] Dependency update

## Related Issues

<!-- Link related issues using "Fixes #123", "Closes #456", "Relates to #789" -->

## Testing

<!-- Describe how you tested your changes -->

### Test Checklist

- [ ] Unit tests pass (`cargo test`)
- [ ] Integration tests pass (`cargo test --test integration`)
- [ ] E2E tests pass (`cargo test --test e2e`)
- [ ] SQLite tests pass
- [ ] PostgreSQL tests pass
- [ ] Frontend builds (`cd frontend && npm run build`)
- [ ] Manual testing performed

### Test Details

```bash
# Commands run and their output
```

## Documentation

- [ ] Code comments updated
- [ ] Rust doc comments updated
- [ ] README updated (if user-facing changes)
- [ ] Architecture docs updated (if architectural changes)
- [ ] Configuration docs updated (if config changes)
- [ ] API reference updated (if API changes)
- [ ] CHANGELOG updated (if applicable)

## Breaking Changes

<!-- If this is a breaking change, describe the impact and migration path -->

## Screenshots

<!-- If UI changes, add screenshots -->

## Checklist

- [ ] My code follows the project's style guidelines
- [ ] I have run `cargo fmt --check` locally
- [ ] I have run `cargo clippy -D warnings` locally
- [ ] I have run `cargo test` locally
- [ ] I have run `cargo audit` locally
- [ ] My changes generate no new warnings
- [ ] I have added tests that prove my fix/feature works
- [ ] New and existing tests pass locally
- [ ] Any dependent changes have been merged and published