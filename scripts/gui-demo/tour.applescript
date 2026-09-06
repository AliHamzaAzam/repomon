-- Only the demo process is targeted. Semantic AX buttons and the current keymap drive the tour.
property demoPID : 0
property tourStart : missing value

on activateDemo()
    tell application "System Events"
        repeat 40 times
            set demoProcesses to (application processes whose unix id is demoPID)
            if (count of demoProcesses) > 0 then
                tell item 1 of demoProcesses
                    set frontmost to true
                    if exists front window then return
                end tell
            end if
            delay 0.5
        end repeat
    end tell
    error "Demo app did not expose an accessible window. Check Accessibility permission for the invoking terminal."
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
    my activateDemo()
    repeat 12 times
        tell application "System Events"
            tell (first process whose unix id is demoPID)
                set nodes to entire contents of front window
                repeat with node in nodes
                    try
                        if role of node is "AXButton" or role of node is "AXRadioButton" then
                            set nodeLabel to description of node as text
                            if nodeLabel is "" or nodeLabel is "button" then set nodeLabel to name of node as text
                            if (exactMatch and nodeLabel is labelText) or ((not exactMatch) and nodeLabel contains labelText) then
                                perform action "AXPress" of node
                                delay 0.3
                                return true
                            end if
                        end if
                    end try
                end repeat
            end tell
        end tell
        if optional then return false
        delay 0.25
    end repeat
    error "Required demo button not found: " & labelText
end buttonMatching

on requireText(labelText)
    repeat 12 times
        tell application "System Events"
            tell (first process whose unix id is demoPID)
                set nodes to entire contents of front window
                repeat with node in nodes
                    try
                        if (value of node as text) contains labelText then return true
                    end try
                    try
                        if (description of node as text) contains labelText then return true
                    end try
                    try
                        if (name of node as text) contains labelText then return true
                    end try
                end repeat
            end tell
        end tell
        delay 0.25
    end repeat
    error "Required demo content not found: " & labelText
end requireText

on revealText(labelText)
    my requireText(labelText)
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            set nodes to entire contents of front window
            repeat with node in nodes
                try
                    if role of node is "AXStaticText" and (value of node as text) contains labelText then
                        perform action "AXScrollToVisible" of node
                        return
                    end if
                end try
            end repeat
        end tell
    end tell
    error "Could not scroll required demo content into view: " & labelText
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
    log "[gui-demo] Tour: " & beat
    set waitSeconds to secondsFromStart - ((get current date) - tourStart)
    if waitSeconds < -2 then error "Tour fell behind its recording schedule at " & beat
    if waitSeconds > 0 then delay waitSeconds
end holdUntil

on opening()
    my activateDemo()
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            set position of front window to {40, 40}
            set size of front window to {1440, 900}
            if size of front window is not {1440, 900} then error "Display cannot accommodate the 1440x900 demo window"
        end tell
    end tell
    my buttonMatching("Skip setup", true, true)
    my selectHero()
    my pressKey("1", true)
    my requireText("Repomind")
    my requireText("Running")
    my requireText("Needs you")
end opening

on run argv
    set demoPID to item 1 of argv as integer
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
        my buttonMatching("Hide ", false, false)
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
    -- Settings uses tab buttons; the header Usage button is outside its dialog.
    tell application "System Events"
        tell (first process whose unix id is demoPID)
            set nodes to entire contents of front window
            set usageTab to missing value
            repeat with node in nodes
                try
                    if role of node is "AXRadioButton" and name of node is "Usage" then set usageTab to node
                end try
            end repeat
            if usageTab is missing value then error "Settings Usage tab not found"
            perform action "AXPress" of usageTab
        end tell
    end tell
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
