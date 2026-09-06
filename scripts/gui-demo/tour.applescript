-- Only the demo process is targeted. Semantic AX buttons and the current keymap drive the tour.
use scripting additions

property demoPID : 0
property tourStart : missing value
property outputDirectory : ""
property tourPhase : "opening"
property lookupSeconds : 12
property readinessSeconds : 30

-- Read each AX field independently. WebKit may put visible text in name or value while
-- description still contains a generic role label. CSS truncation is not a selector.
on nodeFields(node)
    set nodeRole to ""
    set nodeDescription to ""
    set nodeName to ""
    set nodeValue to ""
    tell application "System Events"
        try
            set nodeRole to role of node as text
        end try
        try
            set nodeDescription to description of node as text
        end try
        try
            set nodeName to name of node as text
        end try
        try
            set nodeValue to value of node as text
        end try
    end tell
    return {nodeRole, nodeDescription, nodeName, nodeValue}
end nodeFields

on fieldsMatch(fields, labelText, exactMatch)
    repeat with fieldNumber from 2 to 4
        set fieldText to item fieldNumber of fields
        if exactMatch then
            if fieldText is labelText then return true
        else
            if fieldText contains labelText then return true
        end if
    end repeat
    return false
end fieldsMatch

-- System Events' "entire contents" stops a few levels into a WKWebView (it returned 96 nodes,
-- most without a role, for a fleet of eight lanes), so walk the tree ourselves, depth first.
on collectNodes(node, acc, depth)
    if depth > 40 then return acc
    set end of acc to node
    set kids to {}
    try
        tell application "System Events" to set kids to UI elements of node
    end try
    repeat with kid in kids
        set acc to my collectNodes(kid, acc, depth + 1)
    end repeat
    return acc
end collectNodes

on windowNodes()
    set acc to {}
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            if unix id is not demoPID then error "AX process reference resolved to a different PID"
            set demoWindows to windows
        end tell
    end tell
    -- WKWebView popups can expose a tiny native dialog as front window. The real
    -- page and its portaled controls remain in the main window's AX tree.
    repeat with rootWindow in demoWindows
        set acc to my collectNodes(rootWindow, acc, 0)
    end repeat
    return acc
end windowNodes

on mainWindow()
    set largestWindow to missing value
    set largestArea to 0
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            if unix id is not demoPID then error "AX main-window lookup resolved to a different PID"
            repeat with candidate in windows
                set dimensions to size of candidate
                set candidateArea to (item 1 of dimensions) * (item 2 of dimensions)
                if candidateArea > largestArea then
                    set largestArea to candidateArea
                    set largestWindow to contents of candidate
                end if
            end repeat
        end tell
    end tell
    if largestWindow is missing value then error "No demo main window"
    return largestWindow
end mainWindow

on flatText(rawText)
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to {return, linefeed, tab}
    set parts to text items of rawText
    set AppleScript's text item delimiters to " "
    set resultText to parts as text
    set AppleScript's text item delimiters to savedDelimiters
    return resultText
end flatText

on describeNode(node)
    set fields to my nodeFields(node)
    return "role=" & item 1 of fields & tab & "description=" & my flatText(item 2 of fields) & tab & "name=" & my flatText(item 3 of fields) & tab & "value=" & my flatText(item 4 of fields)
end describeNode

on dumpAccessibility(reasonText)
    set dumpLines to {"phase=" & tourPhase & "; pid=" & demoPID & "; reason=" & reasonText}
    try
        set nodes to my windowNodes()
        set end of dumpLines to "total nodes=" & (count of nodes)
        -- Every role, so a row exposed as a group, list item, cell or link is visible in the dump.
        repeat with node in nodes
            if (count of dumpLines) > 600 then exit repeat
            set end of dumpLines to my describeNode(node)
        end repeat
    on error dumpError
        set end of dumpLines to "AX traversal failed: " & dumpError
    end try
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to linefeed
    set dumpText to (dumpLines as text) & linefeed
    set AppleScript's text item delimiters to savedDelimiters
    set dumpFile to missing value
    try
        set dumpFile to open for access (POSIX file (outputDirectory & "/ax-dump.txt")) with write permission
        set eof of dumpFile to 0
        write dumpText to dumpFile as «class utf8»
        close access dumpFile
    on error writeError
        try
            if dumpFile is not missing value then close access dumpFile
        end try
        log "[gui-demo] Could not write AX dump: " & writeError
    end try
    log "[gui-demo] AX dump: " & outputDirectory & "/ax-dump.txt (first 40 lines follow)"
    set previewCount to count of dumpLines
    if previewCount > 40 then set previewCount to 40
    repeat with lineNumber from 1 to previewCount
        log "[gui-demo] AX " & item lineNumber of dumpLines
    end repeat
end dumpAccessibility

on lookupFailure(reasonText)
    my dumpAccessibility(reasonText)
    error reasonText
end lookupFailure

-- Prefer the button's own fields. If WebKit exposes a lane's name only as an AXStaticText,
-- follow AXParent to its enclosing button; never click an unrelated current/selected row.
on parentNode(node)
    tell application "System Events" to return value of attribute "AXParent" of node
end parentNode

on findButton(nodes, labelText, exactMatch, wantedRole)
    repeat with node in nodes
        set fields to my nodeFields(node)
        set nodeRole to item 1 of fields
        if (nodeRole is "AXButton" or nodeRole is "AXRadioButton" or nodeRole is "AXCheckBox" or nodeRole is "AXPopUpButton") and (wantedRole is "" or nodeRole is wantedRole) then
            if my fieldsMatch(fields, labelText, exactMatch) then return {contents of node, "button fields"}
        end if
    end repeat
    repeat with node in nodes
        set fields to my nodeFields(node)
        if item 1 of fields is "AXStaticText" and my fieldsMatch(fields, labelText, exactMatch) then
            set ancestor to contents of node
            repeat 12 times
                try
                    set ancestor to my parentNode(ancestor)
                    set parentFields to my nodeFields(ancestor)
                    set parentRole to item 1 of parentFields
                    if (parentRole is "AXButton" or parentRole is "AXRadioButton" or parentRole is "AXCheckBox" or parentRole is "AXPopUpButton") and (wantedRole is "" or parentRole is wantedRole) then
                        return {ancestor, "static text child: " & my describeNode(node)}
                    end if
                    if parentRole is "AXWindow" or parentRole is "AXApplication" then exit repeat
                on error
                    exit repeat
                end try
            end repeat
        end if
    end repeat
    return missing value
end findButton

on pressFound(found, labelText)
    set targetNode to item 1 of found
    log "[gui-demo] Match " & labelText & " via " & item 2 of found & "; " & my describeNode(targetNode)
    tell application "System Events" to perform action "AXPress" of targetNode
    delay 0.3
end pressFound

on waitForFleet()
    set readinessStart to current date
    set lastError to "no fleet text yet"
    set skipSeen to false
    repeat
        try
            set nodes to my windowNodes()
            -- Check the expected fleet first. An absent Skip setup search traverses
            -- the whole tree twice and previously consumed 34 seconds on a ready fleet.
            repeat with node in nodes
                set fields to my nodeFields(node)
                if item 1 of fields is "AXStaticText" and my fieldsMatch(fields, "orbit-api", false) then
                    log "[gui-demo] Fleet ready after " & ((current date) - readinessStart) & " s; onboarding skipped=" & skipSeen
                    return
                end if
            end repeat
            set skipButton to my findButton(nodes, "Skip setup", true, "")
            if skipButton is not missing value then
                set skipSeen to true
                log "[gui-demo] Onboarding detected; pressing Skip setup in the isolated app"
                my pressFound(skipButton, "Skip setup")
            end if
        on error readinessError
            set lastError to readinessError
        end try
        if ((current date) - readinessStart) >= readinessSeconds then exit repeat
        delay 0.5
    end repeat
    my lookupFailure("Fleet not ready after " & ((current date) - readinessStart) & " s: AXStaticText orbit-api missing; onboarding seen=" & skipSeen & "; " & lastError)
end waitForFleet

on activateDemo()
    tell application "System Events"
        repeat 40 times
            set demoProcesses to (application processes whose unix id is demoPID)
            if (count of demoProcesses) > 0 then
                tell item 1 of demoProcesses
                    if unix id is not demoPID then error "AX activation resolved to a different PID"
                    set frontmost to true
                    if exists front window then return
                end tell
            end if
            delay 0.5
        end repeat
    end tell
    my lookupFailure("Demo app did not expose an accessible window. Check Accessibility permission for the invoking terminal.")
end activateDemo

on pressKey(k, shifted)
    my activateDemo()
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            if shifted then
                keystroke k using {command down, shift down}
            else
                keystroke k using {command down}
            end if
        end tell
    end tell
    delay 0.25
end pressKey

on plainKey(k)
    my activateDemo()
    tell application "System Events" to tell (first process whose unix id is demoPID) to keystroke k
end plainKey

on escapeKey()
    my activateDemo()
    tell application "System Events" to tell (first process whose unix id is demoPID) to key code 53
    delay 0.25
end escapeKey

on buttonMatching(labelText, exactMatch, optional)
    return my buttonWithRole(labelText, exactMatch, optional, "")
end buttonMatching

on pickerButton(labelText)
    set lookupStart to current date
    repeat
        repeat with node in my windowNodes()
            set fields to my nodeFields(node)
            if my fieldsMatch(fields, "Choose multitasking panes", true) then
                set pickerNodes to my collectNodes(node, {}, 0)
                set found to my findButton(pickerNodes, labelText, false, "")
                if found is not missing value then
                    my pressFound(found, labelText)
                    return
                end if
            end if
        end repeat
        if ((current date) - lookupStart) >= lookupSeconds then exit repeat
        delay 0.5
    end repeat
    my lookupFailure("Required pane-picker control missing: " & labelText)
end pickerButton

on buttonWithRole(labelText, exactMatch, optional, wantedRole)
    my activateDemo()
    set lookupStart to current date
    set lastError to "no matching AX button"
    repeat
        try
            set found to my findButton(my windowNodes(), labelText, exactMatch, wantedRole)
            if found is not missing value then
                my pressFound(found, labelText)
                return true
            end if
        on error pressError
            set lastError to pressError
        end try
        if ((current date) - lookupStart) >= lookupSeconds then exit repeat
        delay 0.5
    end repeat
    set reasonText to "Required demo button not found: " & labelText & " after " & ((current date) - lookupStart) & " s; " & lastError
    if optional then
        my dumpAccessibility(reasonText)
        return false
    end if
    my lookupFailure(reasonText)
end buttonWithRole

on requireText(labelText)
    set lookupStart to current date
    repeat
        try
            repeat with node in my windowNodes()
                if my fieldsMatch(my nodeFields(node), labelText, false) then return true
            end repeat
        end try
        if ((current date) - lookupStart) >= lookupSeconds then exit repeat
        delay 0.5
    end repeat
    my lookupFailure("Required demo content not found: " & labelText)
end requireText

on revealText(labelText)
    my requireText(labelText)
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            set nodes to my windowNodes()
            repeat with node in nodes
                try
                    if role of node is "AXStaticText" and my fieldsMatch(my nodeFields(node), labelText, false) then
                        perform action "AXScrollToVisible" of node
                        return
                    end if
                end try
            end repeat
        end tell
    end tell
    my lookupFailure("Could not scroll required demo content into view: " & labelText)
end revealText

on selectHero()
    my buttonMatching("nav-focus-trap", false, false)
end selectHero

on findFile(fileName)
    my pressKey("p", false)
    my plainKey(fileName)
    delay 0.5
end findFile

on openFoundFile()
    tell application "System Events" to tell (first process whose unix id is demoPID) to key code 36
    delay 0.4
end openFoundFile

on holdUntil(secondsFromStart, beat)
    set tourPhase to beat
    log "[gui-demo] Tour at " & ((get current date) - tourStart) & " s: " & beat
    set waitSeconds to secondsFromStart - ((get current date) - tourStart)
    -- AX calls vary with tree size. Preserve a readable dwell on every verified view;
    -- the recorder follows tour completion instead of cutting off at a fixed deadline.
    if waitSeconds < 3 then set waitSeconds to 3
    if waitSeconds > 0 then delay waitSeconds
end holdUntil

on opening()
    my activateDemo()
    set demoWindow to my mainWindow()
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            -- Top-left corner: macOS clamps the window under the menu bar and above the Dock, so
            -- a 1440x900 request on a 1512x982 laptop display may come back shorter. Accept the
            -- largest size that fits; the recorder crops to the window's real bounds.
            set position of demoWindow to {0, 0}
            set size of demoWindow to {1440, 900}
            set actualSize to size of demoWindow
            if (item 1 of actualSize) < 1280 or (item 2 of actualSize) < 760 then error "Display cannot accommodate a demo window of at least 1280x760 (got " & (item 1 of actualSize) & "x" & (item 2 of actualSize) & "); hide the Dock or use a larger display"
            log "demo window size " & (item 1 of actualSize) & "x" & (item 2 of actualSize)
        end tell
    end tell
    my waitForFleet()
    my selectHero()
    my pressKey("1", true)
    my requireText("Repomind")
    my requireText("Running")
    my requireText("Needs you")
end opening

on run argv
    set demoPID to item 1 of argv as integer
    set outputDirectory to item 3 of argv
    set tourPhase to item 2 of argv
    if item 2 of argv is "opening" then
        my opening()
        return
    end if
    set tourStart to current date
    my holdUntil(8, "Fleet and fake Claude hero")

    my pressKey("5", false)
    my buttonMatching("Configure multitasking panes", true, false)
    -- Default selection has six panes. Remove two through the picker, then verify four.
    repeat 2 times
        my pickerButton("Hide ")
    end repeat
    my escapeKey()
    my requireText("4 panes")
    my holdUntil(16, "Multitasking: four panes")
    my pressKey("5", false)

    my selectHero()
    my pressKey("1", false)
    my buttonMatching("MobileMenu.tsx", false, false)
    my requireText("Restore focus")
    my holdUntil(24, "Git: uncommitted navigation diff")
    my pressKey("1", false)

    my buttonMatching("Editor", true, false)
    my findFile("MobileMenu.tsx")
    my openFoundFile()
    my requireText("MobileMenu.tsx")
    my holdUntil(32, "Editor workspace: TypeScript syntax")
    my findFile("useFocusTrap")
    my requireText("useFocusTrap")
    my holdUntil(38, "File finder")
    my escapeKey()
    my findFile("navigation.svg")
    my openFoundFile()
    my pressKey("v", true)
    my holdUntil(44, "Image preview: navigation design")
    my buttonMatching("Editor", true, false)

    my pressKey("3", false)
    my buttonMatching("7 days", true, false)
    my requireText("Sessions")
    my holdUntil(49, "Usage chart and cards")
    my revealText("Sessions")
    my holdUntil(53, "Usage: seven-day chart, cards and sessions")
    my pressKey(",", false)
    -- Restrict the match to the radio/tab role so the toolbar Usage button cannot win.
    my buttonWithRole("Usage", true, false, "AXRadioButton")
    my requireText("Model")
    my holdUntil(61, "Settings: model rates")
    my escapeKey()
    my pressKey("3", false)

    my pressKey("8", false)
    my requireText("Rate-limit headers are ready")
    my holdUntil(68, "Repomail: coordinated fake lanes")
    my buttonMatching("rate-limit-headers", false, false)
    my pressKey("7", false)
    my revealText("Activity log")
    my requireText("hold")
    my holdUntil(75, "Supervision: held permission audit")
    my pressKey("9", false)
    my requireText("release-review")
    my holdUntil(82, "Repomind: two plans and a draft playbook")
    my pressKey("9", false)
    my pressKey("/", true)
    my requireText("Keyboard shortcuts")
    my holdUntil(84, "Shortcuts overlay")
    my escapeKey()
    my selectHero()
    my holdUntil(90, "Return to the opening hero")
end run
