_kubectl__view_storage() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="kubectl__view_storage"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        kubectl__view_storage)
            opts="-n -A -u -o -l -g -h -V --build-info --completions --namespace --all-namespaces --context --kubeconfig --utilization --metrics-source --prometheus-url --prometheus-token-file --prometheus-ca-file --prometheus-cluster --prometheus-single-cluster --view --wide --output --kind --selector --pvc --pv --storage-class --driver --node --statefulset --workload --compare --watch --watch-count --request-timeout --timeout --concurrency --max-metrics-age --warn-used --critical-used --require-metrics --fail-on --color --group-by --units --sort --help --version"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --completions)
                    COMPREPLY=($(compgen -W "bash zsh fish" -- "${cur}"))
                    return 0
                    ;;
                --namespace)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -n)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --context)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --kubeconfig)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --metrics-source)
                    COMPREPLY=($(compgen -W "kubelet prometheus" -- "${cur}"))
                    return 0
                    ;;
                --prometheus-url)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prometheus-token-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prometheus-ca-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prometheus-cluster)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --view)
                    COMPREPLY=($(compgen -W "tree table summary issues stats" -- "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "text json csv" -- "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -W "text json csv" -- "${cur}"))
                    return 0
                    ;;
                --kind)
                    COMPREPLY=($(compgen -W "pvc pv" -- "${cur}"))
                    return 0
                    ;;
                --selector)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -l)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pvc)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pv)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --storage-class)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --driver)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --node)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --statefulset)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --workload)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --compare)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --watch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --watch-count)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --request-timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --concurrency)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --max-metrics-age)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --warn-used)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --critical-used)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --fail-on)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --color)
                    COMPREPLY=($(compgen -W "auto always never" -- "${cur}"))
                    return 0
                    ;;
                --group-by)
                    COMPREPLY=($(compgen -W "storageclass namespace pvc pv driver pod node accessmode volumemode phase statefulset workload" -- "${cur}"))
                    return 0
                    ;;
                -g)
                    COMPREPLY=($(compgen -W "storageclass namespace pvc pv driver pod node accessmode volumemode phase statefulset workload" -- "${cur}"))
                    return 0
                    ;;
                --units)
                    COMPREPLY=($(compgen -W "auto B KB MB GB TB PB KiB MiB GiB TiB PiB" -- "${cur}"))
                    return 0
                    ;;
                --sort)
                    COMPREPLY=($(compgen -W "name requested provisioned used available used-percent inode-percent severity name:asc name:desc requested:asc requested:desc provisioned:asc provisioned:desc used:asc used:desc available:asc available:desc used-percent:asc used-percent:desc inode-percent:asc inode-percent:desc severity:asc severity:desc" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _kubectl__view_storage -o nosort -o bashdefault -o default kubectl-view_storage
else
    complete -F _kubectl__view_storage -o bashdefault -o default kubectl-view_storage
fi
