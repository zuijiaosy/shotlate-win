on run argv
	set cmdline to item 1 of argv
	with timeout of 600 seconds
		tell application "UTM"
			set vm to virtual machine named "Shotlate Win11"
			set p to execute vm at "cmd.exe" with arguments {"/c", cmdline} with output capturing
			repeat 1200 times
				set r to get result p
				if exited of r then exit repeat
				delay 0.5
			end repeat
			return (output data of r) & "|" & (error data of r) & "|" & (exit code of r)
			return "
[exit " & (exit code of r) & "]"
		end tell
	end timeout
end run
