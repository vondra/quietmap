//! Test fixture: a present module whose own dependency is absent — the importer must rethrow, never null.

// Variable specifier: keeps tsc from resolving (and rejecting) the absent file,
// while Node still fails the import at evaluation time.
const missingDependency = './ops-fixture-no-such-dependency.js'
await import(missingDependency)
