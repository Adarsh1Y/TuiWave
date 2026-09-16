# bash completion for lastwave
# Usage: source completions/lastwave.bash

_lastwave() {
    local cur prev words cword
    _init_completion || return

    local opts=(
        -v --volume
        --mpv-path
        --resume
        --no-resume
        --play
        -h --help
        -V --version
    )

    case "$prev" in
        -v|--volume)
            # 0-150; numbers only
            COMPREPLY=($(compgen -W "$(seq 0 15)" -- "$cur"))
            return
            ;;
        --mpv-path|--play)
            _filedir
            return
            ;;
    esac

    if [[ $cur == -* ]]; then
        COMPREPLY=($(compgen -W "${opts[*]}" -- "$cur"))
    else
        _filedir
    fi
}

complete -F _lastwave lastwave