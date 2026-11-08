{
  schema_version,
  selection,
  records: [.records[] | {
    name, phase, requested_bytes, provisioned_bytes,
    filesystem: {
      status: .filesystem.status,
      capacity_bytes: .filesystem.capacity_bytes,
      used_bytes: .filesystem.used_bytes,
      available_bytes: .filesystem.available_bytes
    },
    inodes: {
      status: .inodes.status,
      inodes_total: .inodes.inodes_total,
      inodes_used: .inodes.inodes_used,
      inodes_free: .inodes.inodes_free
    }
  }]
}
