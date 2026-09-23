# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

## usage: . scripts/fx-env.sh [--bash-build-completions]
##
## optional arguments:
##   --bash-build-completion  Enables BASH completion support for the `fx build`
##                            command. See //scripts/gn_complete/README.md for
##                            other requirements.

### NOTE!
###
### This is not a normal shell script that executes on its own.
###
### It's evaluated directly in a user's interactive shell using
### the `.` or `source` shell built-in.
###
### Hence, this code must be careful not to pollute the user's shell
### with variable or function symbols that users don't want.

function __fx_env_main() {
  local bash_build_completion=false
  for arg in "$@"; do
    if [[ "${arg}" == "--bash-build-completion" ]]; then
      bash_build_completion=true
    fi
  done

  if [[ -n "${ZSH_VERSION}" ]]; then
    export FUCHSIA_DIR="$(cd "$(dirname "${(%):-%x}")/.." >/dev/null 2>&1 && pwd)"
  else
    export FUCHSIA_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1 && pwd)"
  fi

  export DEBUGINFOD_URLS="$(source "$FUCHSIA_DIR/tools/devshell/lib/vars.sh" && echo -n "$DEBUGINFOD_URLS")"

  # __update_path <suffix> <suffix>...
  # Removes old $PATH members who match any of the suffixes and then adds them
  # back prefixed by $FUCHSIA_DIR.
  function __update_path {
    if [[ -n "${ZSH_VERSION}" ]]; then
      local s
      for s in $*; do
        path[$path[(i)*/$s]]=("$FUCHSIA_DIR/$s")
      done
    else
      local -a path
      local s i found
      IFS=':' read -r -a path <<< "$PATH"
      for s in "$@"; do
        found=
        for i in "${!path[@]}"; do
          if [[ ${path[$i]} = */$s ]]; then
            found=yes
            path[$i]="$FUCHSIA_DIR/$s"
            break
          fi
        done
        if [[ -z "$found" ]]; then
          path+=("$FUCHSIA_DIR/$s")
        fi
      done
      PATH="$(IFS=:; echo "${path[*]}")"
    fi
  }

  ### fx-update-path: add useful tools to the PATH

  # Add tools to path, removing prior tools directory if any. This also
  # matches the Zircon tools directory added by zset, so add it back too.
  function fx-update-path {
    local rust_dir="$(source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh" && echo -n "${PREBUILT_RUST_DIR}/bin")"
    __update_path \
      .jiri_root/bin \
      scripts/git \
      ${rust_dir#${FUCHSIA_DIR}/}
    export PATH
  }

  ### fx-prompt-info: prints the current configuration for display in a prompt

  function fx-prompt-info {
    if [[ ${PWD}/ = ${FUCHSIA_DIR}/* ]]; then
      # Run in a subshell to avoid polluting this shell's environment with data
      # from the config file, which can change without updating this shell's
      # environment.
      (
        source "${FUCHSIA_DIR}/tools/devshell/lib/vars.sh"
        if fx-build-dir-if-present; then
          echo "${FUCHSIA_BUILD_DIR##*/}"
        else
          echo "???"
        fi
      )
    fi
  }

  ### fx-set-prompt: displays the current configuration in the prompt

  function fx-set-prompt {
    if [[ -n "${ZSH_VERSION}" ]]; then
      autoload -Uz colors && colors
      setopt PROMPT_SUBST
      export PS1='%B%F{yellow}[$(fx-prompt-info)] %B%F{blue}%m:%~%#%f%b '
      export PS2='%B%F{blue}>%f%b '
    else
      export PS1='\[\e[0;1;33m\][$(fx-prompt-info)] \[\e[34m\]\h:\w\\$\[\e[0m\] '
      export PS2='\[\e[0;1;34m\]>\[\e[0m\] '
    fi
  }

  ### fdir: navigate to directories with autocomplete

  # $ fdir --help   # for usage.
  function fdir {
    local fd_python
    local dest
    cd "${FUCHSIA_DIR}"
    fd_python="scripts/fd.py"
    dest=$(fuchsia-vendored-python ${fd_python} "$@")
    cd -- "${dest}"
  }

  if [[ -z "${ZSH_VERSION}" ]]; then
    function __fdir {
      local cur
      COMPREPLY=()
      cur="${COMP_WORDS[COMP_CWORD]}"
      if [[ ${cur:0:2} == "//" ]]; then
        COMPREPLY=($(/bin/ls -dp1 ${FUCHSIA_DIR}/${cur}* 2>/dev/null | \
          sed -n "s|^${FUCHSIA_DIR}/\(.*/\)\$|\1|p" | xargs echo))
      else
        COMPREPLY=($(/bin/ls -dp1 ${cur}* 2>/dev/null | grep "/$" | xargs echo))
      fi
    }
    complete -o nospace -F __fdir fdir
  fi

  # Support command-line auto-completions for the fx command.
  if [[ -z "${ZSH_VERSION}" ]]; then

    function __fx_complete_cmd {
      local cmd cur prev
      cmd="${COMP_WORDS[1]}"
      cur="${COMP_WORDS[COMP_CWORD]}"
      prev="${COMP_WORDS[COMP_CWORD-1]}"
      case "${cmd}" in
        set)
          if [[ ${COMP_CWORD} -eq 2 ]]; then
            __fx_set_compreply_for_product_board "${cur}"
            return
          fi
          ;;

        vendor)
          if [[ ${COMP_CWORD} -eq 2 ]]; then
            # return only vendors that have vendor/*/scripts/devshell/*
            COMPREPLY=()
            for v in "${FUCHSIA_DIR}"/vendor/"${cur}"*/scripts/devshell; do
              v=${v##"${FUCHSIA_DIR}/vendor/"}
              v=${v%%"/scripts/devshell"}
              COMPREPLY+=("$v")
            done
            return

          elif [[ ${COMP_CWORD} -eq 3 ]]; then
            COMPREPLY=()
            for file in "${FUCHSIA_DIR}"/vendor/"${prev}"/scripts/devshell/"${cur}"*; do
              if [[ -x "${file}" ]]; then
                COMPREPLY+=("${file##*/}")
              fi
            done
            return
          fi
          ;;

        worktree)
          if [[ ${COMP_CWORD} -eq 2 ]]; then
            COMPREPLY=($(compgen -W "pool locate list add remove" -- "${cur}"))
          elif [[ ${COMP_CWORD} -ge 3 ]]; then
            local subcmd="${COMP_WORDS[2]}"
            case "${subcmd}" in
              pool)
                if [[ ${COMP_CWORD} -eq 3 ]]; then
                  COMPREPLY=($(compgen -W "list add remove" -- "${cur}"))
                elif [[ ${COMP_CWORD} -ge 4 ]]; then
                  local pool_subcmd="${COMP_WORDS[3]}"
                  case "${pool_subcmd}" in
                    add)
                      if [[ "${prev}" == "--set" ]]; then
                        :
                      else
                        COMPREPLY=($(compgen -W "--set" -- "${cur}"))
                      fi
                      ;;
                    remove)
                      COMPREPLY=($(compgen -W "--force $(__fx_worktree_names_filtered physical_all)" -- "${cur}"))
                      ;;
                  esac
                fi
                ;;
              locate)
                COMPREPLY=($(compgen -W "$(__fx_worktree_names_filtered all)" -- "${cur}"))
                ;;
              add)
                if [[ "${prev}" == "--pool-name" ]]; then
                  COMPREPLY=($(compgen -W "$(__fx_worktree_names_filtered physical_free)" -- "${cur}"))
                else
                  COMPREPLY=($(compgen -W "--sync --pool-name --json" -- "${cur}"))
                fi
                ;;
              remove)
                COMPREPLY=($(compgen -W "$(__fx_worktree_names_filtered leased)" -- "${cur}"))
                ;;
            esac
          fi
          ;;

        build)
          if [[ ${COMP_CWORD} -eq 2 ]]; then
            __fx_complete_build "${cur}"
          fi
          ;;
      esac
    }

    # Only define the completion function for `fx build` if requested.
    if $bash_build_completion; then
      function __fx_complete_build {
        local target_py="${FUCHSIA_DIR}/scripts/gn_complete/complete.py"
        # In a subshell, so as not to pollute the user's environment,
        # load the build environment variables and execute the completion
        # script.
        COMPREPLY=(
          $(
            source "${FUCHSIA_DIR}/tools/devshell/lib/vars.sh"
            fx-config-read
            "${target_py}" "${1}" 2> /dev/null
          )
        )
      }
    else
      function __fx_complete_build { :; }
    fi

    function __fx_set_compreply_for_product_board {
      local prefix=$1
      if [[ "${prefix}" =~ \. ]]; then
        # product is filled, find a board
        local product="${prefix%%\.*}"
        prefix="${prefix##*\.}"
        for file in "${FUCHSIA_DIR}"/{.,vendor/*}/boards/"${prefix}"*.gni*; do
          if [[ -f "${file}" ]]; then
            file="${file##*/}"
            COMPREPLY+=("${product}.${file%%".gni"}")
          fi
        done
      else
        # find a product
        if [[ $(type -t compopt) == 'builtin' ]]; then
          compopt -o nospace
        fi
        for file in "${FUCHSIA_DIR}"/{.,vendor/*}/products/"${prefix}"*.gni*; do
          if [[ -f "${file}" ]]; then
            file="${file##*/}"
            COMPREPLY+=("${file%%".gni"}")
          fi
        done
        if [[ ${#COMPREPLY[@]} -eq 1 ]]; then
          COMPREPLY=("${COMPREPLY[0]}.")
        fi
      fi
    }

    function __fx {
      local fuchsia_tools_dir="$(fx-config-read 2>/dev/null; echo "${FUCHSIA_BUILD_DIR}/tools")"
      COMPREPLY=()
      if [[ ${COMP_CWORD} -eq 1 ]]; then
        local files cmd
        cmd="${COMP_WORDS[1]}"
        files=("${FUCHSIA_DIR}"/tools/devshell/"${cmd}"* "${FUCHSIA_DIR}"/tools/devshell/contrib/"${cmd}"*)
        if [[ -d "${fuchsia_tools_dir}" ]]; then
          files+=("${fuchsia_tools_dir}"/"${cmd}"*)
        fi
        for file in "${files[@]}"; do
          if [[ "${file}" =~ .fx$ || ( -f "${file}" && -x "${file}" ) ]]; then
            file="${file%%.fx}"
            COMPREPLY+=("${file##*/}")
          fi
        done
      else
        __fx_complete_cmd
      fi
    }
    complete -o default -F __fx fx
  fi

  # Helper function to generate filtered lists of worktree names for completion in Bash.
  # Supported filters:
  #   - physical_free:   Only physical worktree slots that are not currently leased.
  #   - physical_leased: Only physical worktree slots that are currently leased.
  #   - physical_all:    All physical worktree slots.
  #   - leased:          Only leased task names (symlinks).
  #   - all:             Both physical worktree slots and leased task names (symlinks).

  function __fx_worktree_names_filtered {
    "${FUCHSIA_DIR}/scripts/fuchsia-vendored-python" "${FUCHSIA_DIR}/tools/devshell/worktree/main.py" _complete "$1"
  }

}


# __fx_env_main uses function names that are non-compliant with POSIX, so
# if this script is running in posix-compliant bash mode, we need to turn
# it off temporarily otherwise it will fail.
__fx_was_posix=0
case :$SHELLOPTS: in
  *:posix:*)
    __fx_was_posix=1
    set +o posix
    ;;
esac

__fx_env_main "$@"

if [[ $__fx_was_posix -eq 1 ]]; then
  set -o posix
fi
unset __fx_was_posix __fx_env_main
