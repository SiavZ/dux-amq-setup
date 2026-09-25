// What the browser actually does when the agent's FIRST tab's provider exits
// cleanly while a sibling tab is live: the slot moves to the sibling, the
// exited row is deleted, and the pane has to follow.
//
//   cd tools/preview-env && node measurements/tab-promotion-handoff.js
//
// Not a screenshot scene: it drives a real exit and records a timeline rather
// than framing a picture. It writes its observations and two screenshots beside
// itself. The preview container must be up (`DUX_SRC=<worktree> ./up.sh`).
const fs = require("fs")
const path = require("path")
const {
  api,
  containerSh,
  get,
  goto,
  open,
  setFixture,
  sleep,
  takeOver,
} = require("../screens/lib.js")

const PROJECT_PATH = "/repos/demo-api"
const AGENT = "tab-handoff"
const OUT = __dirname
// The exit is watched for this long, at this cadence, because what is being
// measured is a flash: a disconnected box between the socket's close and the
// spine's promotion would be gone long before a human looked.
const POLL_MS = 50
const WATCH_MS = 5000

async function waitForAgent(name, timeoutMs = 90000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const found = (await get("/api/v1/sessions")).find((s) => s.title === name)
    if (found && found.tabs.length) return found
    await sleep(1000)
  }
  throw new Error(`the agent ${name} never appeared`)
}

async function seed() {
  const projects = await get("/api/v1/projects")
  const project =
    projects.find((p) => p.path === PROJECT_PATH) ??
    (await api("POST", "/api/v1/projects", { path: PROJECT_PATH, name: "demo-api" }))
  await api("PATCH", `/api/v1/projects/${project.id}`, { provider: "fake" })
  // A fresh workspace opens on the welcome dialog, which sits over the pane and
  // takes every keystroke this measurement means for the terminal.
  await api("POST", "/api/v1/first-load/dismiss")

  const existing = (await get("/api/v1/sessions")).find((s) => s.title === AGENT)
  if (existing) {
    await api("DELETE", `/api/v1/sessions/${existing.id}`, { remove_worktree: true })
    await sleep(3000)
  }
  // An earlier run's branch and worktree, which creation refuses to reuse. This
  // script owns both names, so removing them is a reset rather than a guess.
  containerSh(
    `cd ${PROJECT_PATH}
     git worktree remove --force /data/dux/worktrees/demo-api/${AGENT} 2>/dev/null || true
     git worktree prune
     git branch -D ${AGENT} 2>/dev/null || true`,
  )
  // The first tab quits on command; the sibling streams, so it is unmistakably
  // live when the first one goes.
  await setFixture("quit-on-command")
  await api("POST", "/api/v1/sessions", {
    kind: "new",
    project_id: project.id,
    name: AGENT,
    copy_uncommitted_changes: false,
  })
  const session = await waitForAgent(AGENT)
  await setFixture("live")
  await api("POST", `/api/v1/sessions/${session.id}/tabs`, { provider: "fake" })
  await sleep(4000)
  const now = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
  if (now.tabs.length !== 2) throw new Error(`expected two tabs, got ${now.tabs.length}`)
  return now
}

// Everything the page says about the exit, read in one pass so a sample is one
// moment rather than four.
const sample = (page) =>
  page.evaluate(() => {
    const text = (el) => (el ? (el.textContent || "").trim() : null)
    const toaster = document.querySelector("[data-sonner-toaster]")
    return {
      connectionLost: document.body.innerText.includes("Connection lost."),
      reconnect: [...document.querySelectorAll("button")].some(
        (b) => (b.textContent || "").trim() === "Reconnect",
      ),
      cover: [...document.querySelectorAll("button")].some((b) =>
        /take over/i.test(b.textContent || ""),
      ),
      strip: [...document.querySelectorAll('[role="tab"], [data-tab-id]')].map((el) => ({
        label: text(el),
        active:
          el.getAttribute("aria-selected") === "true" ||
          el.getAttribute("data-active") === "true",
      })),
      toasts: toaster ? [...toaster.querySelectorAll("li")].map((li) => text(li)) : [],
      // The agent's sidebar row, whose second line carries the state word.
      sidebar: [...document.querySelectorAll("span")]
        .map((el) => text(el))
        .filter((t) => t && t.startsWith("tab-handoff") && t.length > "tab-handoff".length)
        .slice(0, 1)[0],
      hash: location.hash,
    }
  })

async function main() {
  const session = await seed()
  const firstTab = session.tabs[0].id
  const sibling = session.tabs[1].id
  const lines = []
  const say = (s) => {
    console.log(s)
    lines.push(s)
  }
  say(`agent ${session.id}: first tab ${firstTab} (${session.tabs[0].provider}), ` +
      `sibling ${sibling} (${session.tabs[1].provider})`)

  const { browser, page } = await open({})
  try {
    await goto(page, `#/agent/${session.id}`)
    await sleep(2000)
    // The agent was created over REST, so this page is a watcher until it
    // presses the card's button; typing is what the exit needs.
    await takeOver(page)
    await sleep(2000)
    const before = await sample(page)
    say(`before: ${JSON.stringify(before)}`)
    await page.screenshot({ path: path.join(OUT, "tab-promotion-before.png") })

    // Type the provider's quit command into the terminal, which is a real
    // keystroke down the same socket a person would use. xterm reads its input
    // from the helper textarea, so that is what the keys are aimed at.
    await page.bringToFront()
    await page.focus(".xterm-helper-textarea")
    const t0 = Date.now()
    await page.type(".xterm-helper-textarea", "quit")
    await page.keyboard.press("Enter")
    say(`typed quit at t+0`)

    const timeline = []
    let lastKey = null
    while (Date.now() - t0 < WATCH_MS) {
      const s = await sample(page)
      const key = JSON.stringify([s.connectionLost, s.reconnect, s.cover, s.hash, s.toasts])
      if (key !== lastKey) {
        timeline.push({ at: Date.now() - t0, ...s })
        lastKey = key
      }
      await sleep(POLL_MS)
    }
    for (const t of timeline) say(`t+${t.at}ms ${JSON.stringify(t)}`)

    const after = await sample(page)
    say(`after: ${JSON.stringify(after)}`)
    await page.screenshot({ path: path.join(OUT, "tab-promotion-after.png") })
    const spine = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
    say(`spine after: slot_tab_id=${spine.slot_tab_id} tabs=${JSON.stringify(
      spine.tabs.map((t) => [t.id, t.provider, t.has_live_process]),
    )} status=${spine.status}`)
    say(`sibling promoted: ${spine.slot_tab_id === sibling}`)
    fs.writeFileSync(path.join(OUT, "tab-promotion-handoff.log"), lines.join("\n") + "\n")
  } finally {
    await browser.close()
  }
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
