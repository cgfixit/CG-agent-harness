# Google search, URL fetch and permitted-page research

Google listings, permitted URL fetch, and page research. Index: [setup-guide.md](../setup-guide.md). TLS, roles, and URL grants: [SECURE_RESEARCH.md](SECURE_RESEARCH.md).

## 7.6 Google search, URL fetch and permitted-page research

The bundled app and browser console use the same backend; no terminal is needed.
Fresh web settings are on, with an empty URL allowlist. Existing settings remain
unchanged. Administrators grant URLs; operators can use granted reads. Enter
slash commands as plain text starting with `/`, without Markdown backticks.

For the keyless Google fallback only, also allow `https://www.google.com/*`.
SerpAPI search needs no page grant. To fetch the example page:

```text
/web allow https://doc.rust-lang.org/book/ch01-01-installation.html
```

Now ask in ordinary chat:

```text
Search Google for the top 5 results for "veeam software cve".
Read https://doc.rust-lang.org/book/ch01-01-installation.html and tell me which command checks the installed Rust compiler version.
```

The model receives `web_search` and `web_fetch` tools. It must support standard
OpenAI-compatible tool calls. Successful tool calls show source links and
outcomes; failures are shown explicitly. The tools cannot grant URLs, change
accounts or keys, run a shell, or write a repository. `/loop` has no web tools.

**Google API setup:** open **API Keys** in the left pane, paste a **SerpAPI** key
into **Google results (SerpAPI)**, save, then click **Test Google search**.
The saved key applies immediately; SerpAPI listings need no Google URL grant. This is a SerpAPI
Google-results key, not a Google Cloud/Custom Search key. Obtain it through
[SerpAPI](https://serpapi.com/search-api). Saved/active masks and restart status
are displayed separately. Existing process environment values take precedence.
No key belongs in the chat transcript or `config.yaml`.

With no active `SERPAPI_API_KEY`, Google search attempts the public Google page.
That fallback has no API key requirement, but can return a JavaScript challenge,
CAPTCHA, redirect or unreadable page. The app reports the failure; it does not
solve challenges, execute the page's JavaScript or manufacture five results.
A configured key's authentication/quota/network error stays an API error; it
does not silently switch to public Google. Clearing a saved SerpAPI key applies immediately before fallback becomes active.

Grant `https://www.google.com/*` for the generated Google search URLs. A grant
for the exact homepage is insufficient. `https://google.com/*` does not grant
`www`; the apex endpoint may redirect and redirects remain refused. Path
wildcards permit query strings while retaining scheme/host/port/path boundaries.
Exact grants retain exact query identity. Search listings contain provider
snippets and ranked links, not fetched destination content. To read a linked
page, independently grant that destination (or an appropriate explicit wildcard).

Direct commands:

```text
/web search veeam software cve
/web fetch https://doc.rust-lang.org/book/ch01-01-installation.html
/web allow https://example.com/docs/* docs https://example.com/docs/
/web allow https://example.com/robots.txt docs
/web pages group=docs widget_open
/web research group=docs How does widget_open fail?
/web cancel
/web inject
```

`/web pages` retains bounded discovery and BM25 passage retrieval; the legacy
`/web search group=docs ...` form also selects page search. `/web research`
retains its dedicated local-model controller and checked quote references.
`/web inject` explicitly selects the last fetched/page-search extract for later
chat; Google listings do not replace that saved selection. `/loop stop` or the
chat cancellation endpoint cancels an in-flight chat tool turn; `/web cancel`
cancels dedicated research.

The authenticated terminal also supports `web search '<keywords>' --count 5`
(default Google), or `web search '<query>' --engine pages --group docs`.
`POST /api/web/search` uses `engine: "google"` for Google and `count: 1..10`;
omitting `engine` preserves legacy page-search API behavior. Direct chat uses
`POST /api/chat` and returns `web_tools` alongside the reply and aggregate usage.

Web chat defaults to at most 3 tool calls (validated `web.chat_tool_calls`, 1–5),
within the chat timeout and web token budget. Fetches retain bounded bytes/time,
public DNS pinning, no proxies/redirects/cookies, and ordinary TLS validation.
Google's fixed API transport shares those network bounds. Permission is checked
before reads and evidence delivery. Search sends the selected query to the
provider, not the complete conversation; retrieved content goes to the local model.

`/web off` suppresses future reads and injection. `/web forget` clears the current
account's selection; `/web deny` revokes a rule. Neither erases previously
recorded conversations. Research/web selections are account scoped and the
public-document cache is shared. See [secure web behavior and evidence](SECURE_RESEARCH.md).
