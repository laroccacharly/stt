# Recording worker

`stt toggle` currently starts a detached `stt record` process and signals it on the next toggle. `record` is an implementation detail, not an intended interactive workflow.

Consider removing the public `record` command and giving `toggle` an internal worker entry point. The worker must remain alive after the first toggle exits, and the next invocation must find and stop it; memory cannot be shared between separate CLI invocations without a long-lived process or other IPC. Preserve PID ownership, cleanup, and the existing record, transcribe, paste, and history flow.
