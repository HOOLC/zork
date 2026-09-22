# Fixed lexer sources

`reference-entry.ts` is the byte-for-byte TypeScript source that produced the retained lexer corpus before the design website was removed. `package.json` is the historical package metadata recovered from the corpus's repeated source period. Both are benchmark inputs, not runnable Web application code. `fixtures/manifest.json` retains the hashes of every generated corpus file; the corpus bytes must not change when the current application or root package changes.
