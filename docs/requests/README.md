# Requests from GOTG

GOTG (`~/Documents/GOTG`, branch `padmap-integration`) is replacing its own
controller handling with padmap, and depends on this repository as a flake
input pinned to a revision. Everything it needs and padmap does not yet do is
written down here, one file per request.

Written by the agent doing that integration, so every one of them is something
that blocked a real launch rather than a wish. Where a file names a path like
`src/client/lib/pads-dolphin.sh`, that is GOTG's tree, and it is named so the
existing implementation can be read rather than guessed at.

Each file says what GOTG is trying to do, what it does today, why padmap's
current shape does not reach it, and what would be enough. None of them ask
for a redesign; they are the seams an *abstraction layer over emulators*
needs when the thing calling it keeps each emulator in a directory of its own.

## This directory is a to-do list, not an archive

**A file here is open.** Answering a request deletes its file, in the same
commit that answers it. So an empty directory means nothing is waiting, and a
file that is present means somebody is.

That is a deliberate trade. The write-ups were worth keeping while they were
being worked, and they stopped being worth keeping once every one of them
ended in "answered" — twenty files nobody would read again, each of which had
to be opened to find that out. What a request was for does not go anywhere:

* the **commit** that answered it says what was wrong and what was measured
  (`git log --diff-filter=D -- docs/requests/` lists them, newest first);
* the **docs** it changed carry the contract — `docs/EVENTS.md` for anything
  on the socket, `docs/STORIES.md` for what a person at the box is promised;
* `FINDINGS.md` carries the reasoning for anything that was a wound rather
  than a feature, which is most of the bugs filed here.

To read one back in full:

    git log --diff-filter=D --name-only -- docs/requests/   # what went, and when
    git show <commit>^:docs/requests/<file>                 # the text as filed

## Filing one

One file, named for what is wanted rather than for the code it would touch.
Say what GOTG is trying to do, what happens today, why the current shape does
not reach it, and what would be enough. A reproduction beats a description.
Nothing here needs to propose an implementation, and the ones that do say so.
