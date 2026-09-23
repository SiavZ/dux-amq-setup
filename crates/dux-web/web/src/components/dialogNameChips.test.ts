// Every variable a modal shows (a branch, a path, a file, a command, an agent,
// project or terminal name, a pull request number) renders through the one
// `InlineCode` chip, and the chip replaces the quotes that used to delimit it.
// This guard reads the source of every dialog and of the copy builders dialogs
// render, and fails on the two shapes the drift takes: a quote opened right
// before an interpolation, and a hand-rolled monospace span standing in for the
// chip. Toasts and the terminal UI are out of scope.
import { readdirSync, readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const componentsDir = fileURLToPath(new URL(".", import.meta.url))
const libDir = fileURLToPath(new URL("../lib/", import.meta.url))

// The dialogs themselves, found by name so a new one is covered the day it
// lands, plus the non-dialog files whose strings a dialog renders.
function scannedFiles(): { name: string; source: string }[] {
  const dialogs = readdirSync(componentsDir)
    .filter((f) => /Dialog.*\.tsx$/.test(f) && !f.includes(".test."))
    .map((f) => ({ name: `components/${f}`, path: componentsDir + f }))
  const copy = [
    ["components/createAgentDialogView.ts", componentsDir + "createAgentDialogView.ts"],
    ["components/startupLogsCopy.ts", componentsDir + "startupLogsCopy.ts"],
    ["lib/addProjectWarning.ts", libDir + "addProjectWarning.ts"],
    ["lib/checkoutDefaultBranch.ts", libDir + "checkoutDefaultBranch.ts"],
    ["lib/detachAgent.ts", libDir + "detachAgent.ts"],
    ["lib/recreateWorkingCopy.ts", libDir + "recreateWorkingCopy.ts"],
  ].map(([name, path]) => ({ name, path }))
  return [...dialogs, ...copy].map(({ name, path }) => ({
    name,
    source: readFileSync(path, "utf-8"),
  }))
}

// A quote that opens immediately before a JSX expression or element, or a
// straight quote opening immediately before a template interpolation, is a quote
// delimiting a name. A quote around fixed UI words ("Reload config") is not, and
// is not matched.
const QUOTE_BEFORE_NAME = /(&ldquo;|“)\s*[{<]|"\$\{/g
// The ad hoc chip the shared component replaced.
const HAND_ROLLED_CHIP =
  /<span\s+className="[^"]*(font-mono[^"]*break-all|break-all[^"]*font-mono)[^"]*"/g

function nameDelimiterViolations(source: string): string[] {
  return [
    ...[...source.matchAll(QUOTE_BEFORE_NAME)],
    ...[...source.matchAll(HAND_ROLLED_CHIP)],
  ].map((m) => lineAround(source, m.index ?? 0))
}

function lineAround(source: string, index: number): string {
  const start = source.lastIndexOf("\n", index) + 1
  const end = source.indexOf("\n", index)
  return source.slice(start, end === -1 ? undefined : end).trim()
}

// Deliberate exceptions, each with the reason it cannot take the chip. An entry
// that no longer matches anything fails the suite, so the list cannot rot.
const ALLOWED: { file: string; line: string; reason: string }[] = [
  {
    file: "components/StandaloneAgentDialog.tsx",
    line: 'placeholder={`Agent name (optional, defaults to "${standaloneAgentDefaultName(selected)}")`}',
    reason:
      "A placeholder is an attribute and can hold no markup, so the quotes are the only delimiter available.",
  },
]

describe("the name-delimiter detector", () => {
  it("flags every shape of a quoted name and a hand-rolled chip", () => {
    for (const bad of [
      "This removes &ldquo;{name}&rdquo; from dux.",
      "No projects match “{query}”.",
      'A branch named “<span className="break-all">{name}</span>”',
      "`dux will ask \"${label}\" to shut down`",
      '<span className="font-mono break-all">{path}</span>',
      '<span className="break-all font-mono">{path}</span>',
    ]) {
      expect(nameDelimiterViolations(bad), bad).toHaveLength(1)
    }
  })

  it("leaves quoted UI words and the shared chip alone", () => {
    for (const fine of [
      "run “Reload config” afterwards.",
      "Tick “Use randomized pet name” to autofill",
      "Delete <InlineCode>{path}</InlineCode>?",
      '<p className="truncate font-mono text-sm">{destination}</p>',
    ]) {
      expect(nameDelimiterViolations(fine), fine).toEqual([])
    }
  })
})

describe("every name a modal shows is the shared chip", () => {
  const files = scannedFiles()

  it("scans the dialogs and the copy they render", () => {
    expect(files.length).toBeGreaterThan(30)
    expect(files.map((f) => f.name)).toContain("components/DeleteSessionDialog.tsx")
  })

  it("has no quote delimiting a name and no hand-rolled chip", () => {
    const found = files.flatMap(({ name, source }) =>
      nameDelimiterViolations(source)
        .filter(
          (line) => !ALLOWED.some((a) => a.file === name && a.line === line),
        )
        .map((line) => `${name}: ${line}`),
    )
    expect(found).toEqual([])
  })

  it("keeps no stale exception", () => {
    for (const allowed of ALLOWED) {
      const file = files.find((f) => f.name === allowed.file)
      expect(file, allowed.file).toBeDefined()
      expect(nameDelimiterViolations(file!.source), allowed.file).toContain(
        allowed.line,
      )
      expect(allowed.reason.length).toBeGreaterThan(0)
    }
  })
})
