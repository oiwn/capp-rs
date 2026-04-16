# Changelog

## Unreleased

### Added

- Added internal `capp-testkit` workspace crate for reusable local HTTP
  fixtures used by tests and examples.
- Added Hyper/Tokio-backed local fixture server with request logging and
  `/stats` reporting.
- Added built-in local fixture scenarios:
  - `blog`
  - `blog_with_posts(n)`
  - `catalog`
  - `infinite`
- Added `local_blog_crawl` mailbox example that crawls a generated local
  100-post blog and validates coverage against fixture stats.

### Changed

- Reintroduced a realistic crawler example path using the mailbox runtime and a
  local in-process site instead of only external-service demos.
- Expanded test coverage around `capp-testkit`, healthcheck timeout/failure
  behavior, and queue serializer success/error paths.
