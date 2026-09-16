#compdef lastwave
# zsh completion for lastwave
# Usage: fpath+=(/path/to/completions); compinit

_lastwave() {
    local -a opts
    opts=(
        '-v[Start volume (0-150)]:volume:(0 1 2 3 4 5 6 7 8 9 10 25 50 75 100 150)'
        '--volume[Start volume (0-150)]:volume:(0 1 2 3 4 5 6 7 8 9 10 25 50 75 100 150)'
        '--mpv-path[Path to the mpv binary]:binary:_command_names -e'
        '--resume[Restore the previous session on start]'
        '--no-resume[Start fresh, ignoring any saved session]'
        '--play[Play a local file or search query immediately]:arg:_files'
        '-h[Print help]'
        '--help[Print help]'
        '-V[Print version]'
        '--version[Print version]'
    )

    _arguments -s "$opts[@]"
}

if [[ -n "$ZSH_VERSION" ]]; then
    _lastwave "$@"
fi