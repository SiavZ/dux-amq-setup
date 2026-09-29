import { useEffect, useRef } from "react"

// A revisit is the user coming back to what is on screen: the window regaining
// focus, or this page becoming the visible one. Both funnel into one handler,
// because returning to a backgrounded tab commonly fires both, and the handler
// is read through a ref so an inline arrow cannot re-subscribe every render.
//
// dux has no file watcher, so this is one of the events the editor questions
// disk truth on, for the open buffers and for the file tree alike.
export function useRevisit(onRevisit: () => void): void {
  const handlerRef = useRef(onRevisit)
  useEffect(() => {
    handlerRef.current = onRevisit
  })
  useEffect(() => {
    const fire = () => handlerRef.current()
    const onVisibility = () => {
      if (document.visibilityState === "visible") fire()
    }
    window.addEventListener("focus", fire)
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      window.removeEventListener("focus", fire)
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [])
}
