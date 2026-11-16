complete -c kubectl-view_storage -l completions -d 'Print shell completions without reading kubeconfig or contacting a cluster' -r -f -a "bash\t''
zsh\t''
fish\t''"
complete -c kubectl-view_storage -s n -l namespace -r
complete -c kubectl-view_storage -l context -r
complete -c kubectl-view_storage -l kubeconfig -r -F
complete -c kubectl-view_storage -l metrics-source -r -f -a "kubelet\t''
prometheus\t''"
complete -c kubectl-view_storage -l prometheus-url -d 'Explicit Prometheus base URL; Kubernetes credentials are never reused' -r
complete -c kubectl-view_storage -l prometheus-token-file -r -F
complete -c kubectl-view_storage -l prometheus-ca-file -r -F
complete -c kubectl-view_storage -l prometheus-cluster -d 'Exact cluster label selector, e.g. cluster=production' -r
complete -c kubectl-view_storage -l view -r -f -a "tree\t''
table\t''
summary\t''
issues\t''
stats\t''"
complete -c kubectl-view_storage -s o -l output -r -f -a "text\t''
json\t''
csv\t''"
complete -c kubectl-view_storage -l kind -r -f -a "pvc\t''
pv\t''"
complete -c kubectl-view_storage -s l -l selector -r
complete -c kubectl-view_storage -l pvc -r
complete -c kubectl-view_storage -l pv -r
complete -c kubectl-view_storage -l storage-class -r
complete -c kubectl-view_storage -l driver -r
complete -c kubectl-view_storage -l node -r
complete -c kubectl-view_storage -l statefulset -d 'Select claims referenced by live pods controlled by this StatefulSet' -r
complete -c kubectl-view_storage -l workload -d 'Select live claims by verified controller, e.g. deployment/api or cronjob/backup' -r
complete -c kubectl-view_storage -l compare -d 'Earlier JSON report used to calculate observed growth and time to full' -r -F
complete -c kubectl-view_storage -l watch -d 'Refresh after each completed collection; stats keeps the preceding sample' -r
complete -c kubectl-view_storage -l watch-count -d 'Stop after this many watch cycles (otherwise continue until interrupted)' -r
complete -c kubectl-view_storage -l request-timeout -r
complete -c kubectl-view_storage -l timeout -r
complete -c kubectl-view_storage -l concurrency -r
complete -c kubectl-view_storage -l max-metrics-age -r
complete -c kubectl-view_storage -l warn-used -r
complete -c kubectl-view_storage -l critical-used -r
complete -c kubectl-view_storage -l fail-on -r
complete -c kubectl-view_storage -l color -r -f -a "auto\t''
always\t''
never\t''"
complete -c kubectl-view_storage -s g -l group-by -r -f -a "storageclass\t''
namespace\t''
pvc\t''
pv\t''
driver\t''
pod\t''
node\t''
accessmode\t''
volumemode\t''
phase\t''
statefulset\t''
workload\t''"
complete -c kubectl-view_storage -l units -d 'Units for text views and stats CSV: auto, B, KB..PB, or KiB..PiB (case insensitive)' -r -f -a "auto\t''
B\t''
KB\t''
MB\t''
GB\t''
TB\t''
PB\t''
KiB\t''
MiB\t''
GiB\t''
TiB\t''
PiB\t''"
complete -c kubectl-view_storage -l sort -r -f -a "name\t''
requested\t''
provisioned\t''
used\t''
available\t''
used-percent\t''
inode-percent\t''
severity\t''
name:asc\t''
name:desc\t''
requested:asc\t''
requested:desc\t''
provisioned:asc\t''
provisioned:desc\t''
used:asc\t''
used:desc\t''
available:asc\t''
available:desc\t''
used-percent:asc\t''
used-percent:desc\t''
inode-percent:asc\t''
inode-percent:desc\t''
severity:asc\t''
severity:desc\t''"
complete -c kubectl-view_storage -l build-info -d 'Print package version, build revision, compiler, and report schema, then exit'
complete -c kubectl-view_storage -s A -l all-namespaces
complete -c kubectl-view_storage -s u -l utilization
complete -c kubectl-view_storage -l prometheus-single-cluster -d 'Assert that the endpoint contains only the selected Kubernetes cluster'
complete -c kubectl-view_storage -l wide
complete -c kubectl-view_storage -l require-metrics
complete -c kubectl-view_storage -s h -l help -d 'Print help'
complete -c kubectl-view_storage -s V -l version -d 'Print version'
