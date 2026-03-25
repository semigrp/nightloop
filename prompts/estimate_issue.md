You are helping prepare a child Issue for `nightloop`.

Given:
- the Issue body
- the linked source-of-truth docs
- the target change size
- the documentation impact
- the dependencies

Return:
1. recommended model profile: fast / balanced / deep
2. optional exact model override only if strongly justified
3. estimated execution time in minutes for a local Codex run
4. estimation confidence: low / medium / high
5. short reason grounded in scope, verification burden, and likely diff size
6. split recommendation if the task appears likely to exceed the declared diff budget or 120 minutes

Do not widen scope. Do not optimize for completeness over reviewability.
