#pragma once
#include <string>

namespace dbx
{
    // Creates \\.\pipe\<pipeName> (current user only) and serves the core on a reader thread;
    // messages are handled on the calling (XAML UI) thread. Returns false if the pipe cannot be
    // created (another host already serves this session). The host exits when the core disconnects.
    bool StartPipeServer(const std::wstring& pipeName);
}
