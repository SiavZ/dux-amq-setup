// POSIX shell quoting for text the user is invited to paste into a shell. The
// same rule as `dux_core::shell_quote::single_quote`.

/// Wrap in single quotes, closing and reopening around each embedded apostrophe.
/// Inside POSIX single quotes nothing else is special, so nothing else is
/// escaped, and leaving the quotes is the only way to include an apostrophe.
export function singleQuoted(text: string): string {
  return `'${text.replaceAll("'", `'\\''`)}'`
}
