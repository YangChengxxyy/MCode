(component
  (import "mcode:plugin/feature-service@0.0.1" (instance $feature-service
    (type $pack-ids (list string))
    (type $pack-selection-view
      (record (field "selection-stamp" string) (field "pack-ids" $pack-ids)))
    (export "pack-selection-view"
      (type $exported-pack-selection-view (eq $pack-selection-view)))
    (type $pack-service-error
      (enum "invalid-selection" "stale-generation" "limit" "unavailable" "failed"))
    (export "pack-service-error"
      (type $exported-pack-service-error (eq $pack-service-error)))
    (type $activated-pack-set (record (field "selection-stamp" string)))
    (export "activated-pack-set"
      (type $exported-activated-pack-set (eq $activated-pack-set)))
    (type $configured-packs-result
      (result $exported-pack-selection-view (error $exported-pack-service-error)))
    (export "configured-packs" (func (result $configured-packs-result)))
    (type $activate-packs-result
      (result $exported-activated-pack-set (error $exported-pack-service-error)))
    (export "activate-packs"
      (func (param "selection-stamp" string) (result $activate-packs-result)))
    (export "start-task" (func (param "request" string) (result string)))
    (export "poll-task" (func (param "request" string) (result string)))
    (export "cancel-task" (func (param "request" string) (result string)))
  ))
  (alias export $feature-service "configured-packs" (func $configured-packs))
  (core module $service-memory-module
    (memory (export "memory") 1 1024)
    (global $heap (mut i32) (i32.const 4096))
    (data (i32.const 1024) "pack-alpha")
    (data (i32.const 1040) "pack-beta")
    (data (i32.const 1060) "psel1-")
    (func (export "realloc") (param $old i32) (param $old-size i32)
      (param $align i32) (param $new-size i32) (result i32)
      (local $ptr i32)
      global.get $heap
      local.get $align
      i32.const 1
      i32.sub
      i32.add
      i32.const 0
      local.get $align
      i32.sub
      i32.and
      local.tee $ptr
      local.get $new-size
      i32.add
      global.set $heap
      local.get $ptr)
  )
  (core instance $service-memory-instance (instantiate $service-memory-module))
  (alias core export $service-memory-instance "memory" (core memory $service-memory))
  (alias core export $service-memory-instance "realloc" (core func $service-realloc))
  (core func $lower-configured-packs (canon lower (func $configured-packs)
    (memory $service-memory) (realloc $service-realloc)))
  (core instance $service-environment
    (export "memory" (memory $service-memory))
    (export "configured-packs" (func $lower-configured-packs))
  )
  (core module $guest
    (import "mcode:plugin/feature-service@0.0.1" "memory" (memory 1 1024))
    (import "mcode:plugin/feature-service@0.0.1" "configured-packs"
      (func $call-configured-packs (param i32)))
    (export "memory" (memory 0))
    (global $phase (mut i32) (i32.const 0))
    (func $bytes-equal (param $left i32) (param $right i32) (param $len i32)
      (result i32)
      (local $index i32)
      (block $different
        (loop $next
          local.get $index
          local.get $len
          i32.ge_u
          br_if $different
          local.get $left
          local.get $index
          i32.add
          i32.load8_u
          local.get $right
          local.get $index
          i32.add
          i32.load8_u
          i32.ne
          if
            i32.const 0
            return
          end
          local.get $index
          i32.const 1
          i32.add
          local.set $index
          br $next))
      i32.const 1)
    (func $outcome (param $tag i32) (param $variant i32) (result i32)
      i32.const 0
      local.get $tag
      i32.store8
      i32.const 1
      local.get $variant
      i32.store8
      i32.const 0)
    (func $matches-id (param $entry i32) (param $expected i32) (param $len i32)
      (result i32)
      local.get $entry
      i32.load offset=4
      local.get $len
      i32.eq
      if (result i32)
        local.get $entry
        i32.load
        local.get $expected
        local.get $len
        call $bytes-equal
      else
        i32.const 0
      end)
    (func $initialize (param i64) (result i32)
      i32.const 128
      call $call-configured-packs
      i32.const 128
      i32.load8_u
      i32.const 1
      i32.eq
      i32.const 132
      i32.load8_u
      i32.const 1
      i32.eq
      i32.and
      if (result i32)
        i32.const 0
        i32.const 0
        call $outcome
      else
        i32.const 1
        i32.const 2
        call $outcome
      end)
    (func $poll (result i32)
      (local $stamp i32)
      (local $ids i32)
      i32.const 256
      call $call-configured-packs
      i32.const 256
      i32.load8_u
      i32.const 0
      i32.ne
      if
        i32.const 1
        i32.const 2
        call $outcome
        return
      end
      i32.const 260
      i32.load
      local.set $stamp
      i32.const 264
      i32.load
      i32.const 38
      i32.ne
      if
        i32.const 1
        i32.const 2
        call $outcome
        return
      end
      local.get $stamp
      i32.const 1060
      i32.const 6
      call $bytes-equal
      i32.eqz
      if
        i32.const 1
        i32.const 2
        call $outcome
        return
      end
      i32.const 268
      i32.load
      local.set $ids
      i32.const 272
      i32.load
      i32.const 2
      i32.ne
      if
        i32.const 1
        i32.const 2
        call $outcome
        return
      end
      global.get $phase
      i32.eqz
      if
        local.get $ids
        i32.const 1024
        i32.const 10
        call $matches-id
        local.get $ids
        i32.const 8
        i32.add
        i32.const 1040
        i32.const 9
        call $matches-id
        i32.and
        i32.eqz
        if
          i32.const 1
          i32.const 2
          call $outcome
          return
        end
        i32.const 2048
        local.get $stamp
        i32.const 38
        memory.copy
        i32.const 1
        global.set $phase
        i32.const 0
        i32.const 1
        call $outcome
        return
      end
      local.get $ids
      i32.const 1040
      i32.const 9
      call $matches-id
      local.get $ids
      i32.const 8
      i32.add
      i32.const 1024
      i32.const 10
      call $matches-id
      i32.and
      local.get $stamp
      i32.const 2048
      i32.const 38
      call $bytes-equal
      i32.eqz
      i32.and
      if (result i32)
        i32.const 0
        i32.const 0
        call $outcome
      else
        i32.const 1
        i32.const 2
        call $outcome
      end)
    (func $shutdown (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (func $manager-task (param i32 i32) (result i32)
      i32.const 8
      i32.const 0
      i32.store
      i32.const 12
      i32.const 0
      i32.store
      i32.const 8)
    (func $realloc (param i32 i32 i32 i32) (result i32)
      i32.const 1024)
    (export "initialize" (func $initialize))
    (export "poll" (func $poll))
    (export "shutdown" (func $shutdown))
    (export "manager-task" (func $manager-task))
    (export "realloc" (func $realloc))
  )
  (core instance $guest-instance (instantiate $guest
    (with "mcode:plugin/feature-service@0.0.1" (instance $service-environment))
  ))
  (alias core export $guest-instance "memory" (core memory $memory))
  (alias core export $guest-instance "initialize" (core func $core-initialize))
  (alias core export $guest-instance "poll" (core func $core-poll))
  (alias core export $guest-instance "shutdown" (core func $core-shutdown))
  (alias core export $guest-instance "manager-task" (core func $core-manager-task))
  (alias core export $guest-instance "realloc" (core func $realloc))

  (type $initialization-context (record (field "generation" u64)))
  (type $state (enum "ready" "pending" "stopping" "stopped"))
  (type $error-code (enum "invalid-state" "feature-unavailable" "failed"))
  (type $outcome (result $state (error $error-code)))
  (type $initialize-func
    (func (param "context" $initialization-context) (result $outcome)))
  (type $lifecycle-func (func (result $outcome)))
  (func $initialize (type $initialize-func)
    (canon lift (core func $core-initialize) (memory $memory)))
  (func $poll (type $lifecycle-func)
    (canon lift (core func $core-poll) (memory $memory)))
  (func $shutdown (type $lifecycle-func)
    (canon lift (core func $core-shutdown) (memory $memory)))

  (component $lifecycle-shim
    (type (record (field "generation" u64)))
    (import "import-initialization-context" (type (eq 0)))
    (type (enum "ready" "pending" "stopping" "stopped"))
    (import "import-state" (type (eq 2)))
    (type (enum "invalid-state" "feature-unavailable" "failed"))
    (import "import-error-code" (type (eq 4)))
    (type (result 3 (error 5)))
    (type (func (param "context" 1) (result 6)))
    (type (func (result 6)))
    (import "import-initialize" (func (type 7)))
    (import "import-poll" (func (type 8)))
    (import "import-shutdown" (func (type 8)))
    (type (record (field "generation" u64)))
    (export "initialization-context" (type 9))
    (type (enum "ready" "pending" "stopping" "stopped"))
    (export "state" (type 11))
    (type (enum "invalid-state" "feature-unavailable" "failed"))
    (export "error-code" (type 13))
    (type (result 12 (error 14)))
    (type (func (param "context" 10) (result 15)))
    (type (func (result 15)))
    (export "initialize" (func 0) (func (type 16)))
    (export "poll" (func 1) (func (type 17)))
    (export "shutdown" (func 2) (func (type 17)))
  )
  (instance $lifecycle (instantiate $lifecycle-shim
    (with "import-initialize" (func $initialize))
    (with "import-poll" (func $poll))
    (with "import-shutdown" (func $shutdown))
    (with "import-initialization-context" (type $initialization-context))
    (with "import-state" (type $state))
    (with "import-error-code" (type $error-code))
  ))
  (export "mcode:plugin/manager-lifecycle@0.0.1" (instance $lifecycle))

  (type $task-func (func (param "request" string) (result string)))
  (func $manager-start-task (type $task-func)
    (canon lift (core func $core-manager-task) (memory $memory) (realloc $realloc)))
  (func $manager-poll-task (type $task-func)
    (canon lift (core func $core-manager-task) (memory $memory) (realloc $realloc)))
  (func $manager-cancel-task (type $task-func)
    (canon lift (core func $core-manager-task) (memory $memory) (realloc $realloc)))
  (component $tasks-shim
    (type (func (param "request" string) (result string)))
    (import "import-start-task" (func (type 0)))
    (import "import-poll-task" (func (type 0)))
    (import "import-cancel-task" (func (type 0)))
    (type (func (param "request" string) (result string)))
    (export "start-task" (func 0) (func (type 1)))
    (export "poll-task" (func 1) (func (type 1)))
    (export "cancel-task" (func 2) (func (type 1)))
  )
  (instance $tasks (instantiate $tasks-shim
    (with "import-start-task" (func $manager-start-task))
    (with "import-poll-task" (func $manager-poll-task))
    (with "import-cancel-task" (func $manager-cancel-task))
  ))
  (export "mcode:plugin/manager-tasks@0.0.1" (instance $tasks))
)
