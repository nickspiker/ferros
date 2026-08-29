#!/system/bin/sh
# Keep CallRec's grants across boots so recording is fully automatic:
#  - RECORD_AUDIO: capture the call
#  - MANAGE_EXTERNAL_STORAGE: write finished .ogg into the shared Recordings folder
# The all-files view only applies to a process that STARTED with the grant, so restart
# the app once after granting.
until [ "$(getprop sys.boot_completed)" = "1" ]; do sleep 2; done
pm grant com.callrec android.permission.RECORD_AUDIO
appops set com.callrec MANAGE_EXTERNAL_STORAGE allow
kill -9 "$(pidof com.callrec)" 2>/dev/null
