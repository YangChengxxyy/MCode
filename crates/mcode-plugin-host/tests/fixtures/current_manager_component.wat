(component
  (import "mcode:plugin/feature-service@0.0.1" (instance
    (export "start-task" (func (param "request" string) (result string)))
    (export "poll-task" (func (param "request" string) (result string)))
    (export "cancel-task" (func (param "request" string) (result string)))
  ))
  (core module $guest
    (memory (export "memory") 1 1024)
    (func $initialize (param i64) (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (func $poll (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (func $shutdown (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (export "initialize" (func $initialize))
    (export "poll" (func $poll))
    (export "shutdown" (func $shutdown))
  )
  (core instance $guest-instance (instantiate $guest))
  (alias core export $guest-instance "memory" (core memory $memory))
  (alias core export $guest-instance "initialize" (core func $core-initialize))
  (alias core export $guest-instance "poll" (core func $core-poll))
  (alias core export $guest-instance "shutdown" (core func $core-shutdown))

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
)
