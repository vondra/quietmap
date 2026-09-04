//! Test fixture: a present module that fails during evaluation — the importer must rethrow, never null.

throw new Error('ops fixture evaluation failure')
