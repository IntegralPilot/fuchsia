---- MODULE chain_lock ----

EXTENDS TLC, Integers, Sequences
CONSTANTS
  THREADS,
  NULL,
  UNLOCKED,
  INVALID_TOKEN,
  ERR_OK,
  ERR_RETRY,
  ERR_CYCLE_DETECTED,
  ERR_INVALID_STATE

\* To enable symmetry across threads, uncomment this as well as the SYMMETRY
\* statement in the model config.
\* SymmetrySets == Permutations(THREADS)

(* A node set is a finite set representing the state of a graph.  It consists
   of a set of sequences, with the first element equal to the node ID, and the
   second element equal to the downstream node (or NULL).
*)
NodeSet(g) == { << i, g[i] >> : i \in DOMAIN g }

(* AllNodes is a finite set containing all of the node ids in a graph encoded as
   a sequence.  It is basically just the DOMAIN of the sequence (which is why
   the set is finite).
*)
AllNodes(g)== DOMAIN g

(* TerminalNodes is the set of node IDs for which the downstream node is NULL *)
TerminalNodes(g)== { n \in AllNodes(g) : g[n] = NULL }

(* UpstreamNodes is the set of node IDs for which the downstream node is not NULL *)
UpstreamNodes(g)== { n \in AllNodes(g) : g[n] /= NULL }

(* DownstreamLinks is the set of unique downstream links in a graph *)
DownstreamLinks(g)== { g[n] : n \in AllNodes(g) }

(* HeadNodes is the set of nodes in a graph for which there does not exist a node which links to them. *)
HeadNodes(g)== { n \in AllNodes(g) : n \notin DownstreamLinks(g) }

(* Our graph is "closed" iff every node in the graph has either a NULL
   downstream link, or there exists a node in the graph whose ID is equal to the
   node's downstream link *)
GraphIsClosed(g) ==
   /\ \A x \in NodeSet(g): (x[2] = NULL) \/ \E y \in NodeSet(g): x[2] = y[1]

(* Whether or not a path through a graph starting at a given node is acyclic or
   not is computed recursively from the graph definition, a starting node, and a
   set of visited nodes which is initalized to the empty set.

   The path is acyclic if:
   1) The downstream link from the starting node is NULL, OR
   2a) The starting node is not in the visited set, AND
   2b) The path starting from the downstream node is acyclic when recusively
       evaluating with the visited set unioned with the set contianing just the
       starting node.
 *)
RECURSIVE PathIsAcyclicEtc(_, _, _, _)
PathIsAcyclicEtc(g, start, visited, path) ==
  (g[start] = NULL) \/
  ((start \notin visited \/ Print(<< "Cycle Detected",
                                    << "Graph", NodeSet(g) >>,
                                    << "Cycle", Append(path, start) >>
                                  >> , FALSE))
      /\ PathIsAcyclicEtc(g, g[start], visited \union {start}, Append(path, start)))
PathIsAcyclic(g, start) == PathIsAcyclicEtc(g, start, {}, << >>)

(* A graph is acyclic if the paths starting from every node are all acyclic. *)
GraphIsAcyclic(g) ==
  \A start \in DOMAIN g: PathIsAcyclic(g, start)

(* for this spec, a graph is considered valid if it is both closed, and acyclic *)
GraphIsValid(g) == GraphIsClosed(g) /\ GraphIsAcyclic(g)

(* Edge operations are encoded as a tuple of the form << NodeId, NewDownstreamNodeId >>.

   The set of all valid add-edge operations for a graph |g| is the cross of the
   set of terminal nodes with the set of all nodes.  Basically, any node which
   does not already have a downstream link can be linked to any node in the
   graph.

   The set of all valid remove-edge operations for a graph |g| is the cross of
   the set of upstream nodes with { NULL }.  Basically, any node which has a
   downstream link can have that link removed.

   The set of all valid operations is just the union of the two.
*)
AllValidAddEdgeOperations(g) == TerminalNodes(g) \X AllNodes(g)
AllValidRemoveEdgeOperations(g) == UpstreamNodes(g) \X { NULL }
AllValidEdgeOperations(g) == AllValidAddEdgeOperations(g) \union
                             AllValidRemoveEdgeOperations(g)


(* MutatedGraph is an operator which produced g', the graph after op has been
   performed on g
 *)
MutatedGraph(g, op) == [ x \in DOMAIN g |-> IF x = op[1] THEN op[2] ELSE g[x] ]

(*
 +-----+   +-----+   +-----+   +-----+   +-----+
 |  1  |   |  2  |   |  5  |   |  8  |   |  9  |
 +-----+   +-----+   +-----+   +-----+   +-----+
    |         |         |
    +----------         v
         |           +-----+
         v           |  6  |
      +-----+        +-----+
      |  3  |           |
      +-----+           v
         |           +-----+
         v           |  7  |
      +-----+        +-----+
      |  4  |
      +-----+
 *)
INITIAL_PI_GRAPH == << (* 1 *) 3,
                       (* 2 *) 3,
                       (* 3 *) 4,
                       (* 4 *) NULL,
                       (* 5 *) 6,
                       (* 6 *) 7,
                       (* 7 *) NULL,
                       (* 8 *) NULL,
                       (* 9 *) NULL
                    >>

(*--algorithm chain_lock

variables
  graph_state = INITIAL_PI_GRAPH,
  token_generator = 1,
  genlock_state = [ node_id \in AllNodes(graph_state) |-> UNLOCKED ],
  ret_vals = [ t \in THREADS |-> NULL ],
  tokens = [ t \in THREADS |-> INVALID_TOKEN ];

macro UnlockNode(id) begin
  assert genlock_state[id] = tokens[self];
  genlock_state[id] := UNLOCKED;
end macro;

procedure LockNode(id)
variables observed
begin
  (* try to CMPX UNLOCKED -> |gen|, if we succeed, we own the lock and can get out *)
  lock_node_start:
  if genlock_state[id] = UNLOCKED then
    genlock_state[id] := tokens[self];
    ret_vals[self] := << ERR_OK >>;
    return;
  else
    observed := genlock_state[id];
  end if;

  (* Someone else owns the lock.  There are three options to consider here.
     1) Their token id was greater than ours.  We have priority, just spin
        waiting for them to either finish, or encounter our path and back off.
     2) Their token was is less than ours.  They have priority, and may be
        waiting for us to get out of their way.  Back off, releasing our
        lock-path and try again later.
     3) Their token was equal to our token.  We have detected a cycle
        and should abort the operation.
   *)
  lock_node_contested:
  if observed > tokens[self] then
    goto lock_node_start;
  elsif observed < tokens[self] then
    ret_vals[self] := << ERR_RETRY >>;
    return;
  else
    (* Attach the current graph state to the error code.  We will use it to
       assert that if we had reported that a cycle was detected, that there
       actually would have been one had we applied our operation.
     *)
    ret_vals[self] := << ERR_CYCLE_DETECTED, graph_state >>;
    return;
  end if
end procedure;

procedure PerformOperation(op)
variables next_node, locked = << >>;
begin
  (* Obtain a token ID for our operation *)
  get_token:
  tokens[self] := token_generator;
  token_generator := token_generator + 1;

  lock_first_node:
  call LockNode(op[1]);

  lock_first_node_result:
  if ret_vals[self][1] /= ERR_OK then
    goto unlock;
  else
    locked := Append(locked, op[1]);
  end if;

  (* Check to see if our operation is still valid, or if we should fast abort.

     1) If our downstream node has already been set to what we want, we are
        already finished.
     2) If we are adding an edge, but the node already has an edge added, we
        need to abort the operation with "invalid state"

     If everthing looks OK, we need to lock the path up until we reach terminal
     node in the graph, then actually mutate our graph.  The next node to
     visit/lock will be either the edge we plan to add, or the current edge (if
     we plan to delete it)
  *)
  operation_check:
  if graph_state[op[1]] = op[2] then
    (* our return value is already set to OK *)
    goto unlock;
  elsif (graph_state[op[1]] /= NULL) /\ (op[2] /= NULL) then
    ret_vals[self][1] := << ERR_INVALID_STATE >>;
  else
    next_node := IF op[2] /= NULL THEN op[2] ELSE graph_state[op[1]];
  end if;

  lock_path:
  (* OK, now just follow the path until the end, locking as we go. *)
  while next_node /= NULL do
    call LockNode(next_node);
    lock_path_result:
    if ret_vals[self][1] /= ERR_OK then
      goto unlock;
    else
      locked := Append(locked, next_node);
      next_node := graph_state[next_node];
    end if;
  end while;

  (* Success!  Perform our operation. *)
  mutate_graph:
  graph_state[op[1]] := op[2];

  (* Unlock our locked set, front to back *)
  unlock:
  while Len(locked) > 0 do
    UnlockNode(Head(locked));
    locked := Tail(locked);
  end while;

  maybe_spin:
  (* If our last lock-op result was retry, go ahead and try again.  Otherwise,
     we are done, and our result (either OK or CYCLE_DETECTED is already stored
     in our ret_val).
   *)
  if ret_vals[self][1] = ERR_RETRY then
    goto lock_first_node;
  else
    (* If the final result of attempting to perform our operation was
       CYCLE_DETECTED, assert that there actually would have been a cycle in the
       graph if we had executed our operation.
     *)
    assert (ret_vals[self][1] /= ERR_CYCLE_DETECTED) \/
           ~GraphIsAcyclic(MutatedGraph(ret_vals[self][2], op));
    return;
  end if;
end procedure

fair process threads \in THREADS begin
  update_edge:
  with operation \in AllValidRemoveEdgeOperations(INITIAL_PI_GRAPH) do
    call PerformOperation(operation);
  end with;

  done:
  skip;
end process;

end algorithm; *)

\* BEGIN TRANSLATION (chksum(pcal) = "128ab4fe" /\ chksum(tla) = "ac0580b6")
CONSTANT defaultInitValue
VARIABLES graph_state, token_generator, genlock_state, ret_vals,
          tokens, pc, stack, id, observed, op, next_node, locked

vars == << graph_state, token_generator, genlock_state, ret_vals,
           tokens, pc, stack, id, observed, op, next_node, locked >>

ProcSet == (THREADS)

Init == (* Global variables *)
        /\ graph_state = INITIAL_PI_GRAPH
        /\ token_generator = 1
        /\ genlock_state = [ node_id \in AllNodes(graph_state) |-> UNLOCKED ]
        /\ ret_vals = [ t \in THREADS |-> NULL ]
        /\ tokens = [ t \in THREADS |-> INVALID_TOKEN ]
        (* Procedure LockNode *)
        /\ id = [ self \in ProcSet |-> defaultInitValue]
        /\ observed = [ self \in ProcSet |-> defaultInitValue]
        (* Procedure PerformOperation *)
        /\ op = [ self \in ProcSet |-> defaultInitValue]
        /\ next_node = [ self \in ProcSet |-> defaultInitValue]
        /\ locked = [ self \in ProcSet |-> << >>]
        /\ stack = [self \in ProcSet |-> << >>]
        /\ pc = [self \in ProcSet |-> "update_edge"]

lock_node_start(self) == /\ pc[self] = "lock_node_start"
                         /\ IF genlock_state[id[self]] = UNLOCKED
                               THEN /\ genlock_state' = [genlock_state EXCEPT ![id[self]] = tokens[self]]
                                    /\ ret_vals' = [ret_vals EXCEPT ![self] = << ERR_OK >>]
                                    /\ pc' = [pc EXCEPT ![self] = Head(stack[self]).pc]
                                    /\ observed' = [observed EXCEPT ![self] = Head(stack[self]).observed]
                                    /\ id' = [id EXCEPT ![self] = Head(stack[self]).id]
                                    /\ stack' = [stack EXCEPT ![self] = Tail(stack[self])]
                               ELSE /\ observed' = [observed EXCEPT ![self] = genlock_state[id[self]]]
                                    /\ pc' = [pc EXCEPT ![self] = "lock_node_contested"]
                                    /\ UNCHANGED << genlock_state, ret_vals,
                                                    stack, id >>
                         /\ UNCHANGED << graph_state, token_generator,
                                         tokens, op, next_node, locked >>

lock_node_contested(self) == /\ pc[self] = "lock_node_contested"
                             /\ IF observed[self] > tokens[self]
                                   THEN /\ pc' = [pc EXCEPT ![self] = "lock_node_start"]
                                        /\ UNCHANGED << ret_vals, stack, id,
                                                        observed >>
                                   ELSE /\ IF observed[self] < tokens[self]
                                              THEN /\ ret_vals' = [ret_vals EXCEPT ![self] = << ERR_RETRY >>]
                                                   /\ pc' = [pc EXCEPT ![self] = Head(stack[self]).pc]
                                                   /\ observed' = [observed EXCEPT ![self] = Head(stack[self]).observed]
                                                   /\ id' = [id EXCEPT ![self] = Head(stack[self]).id]
                                                   /\ stack' = [stack EXCEPT ![self] = Tail(stack[self])]
                                              ELSE /\ ret_vals' = [ret_vals EXCEPT ![self] = << ERR_CYCLE_DETECTED, graph_state >>]
                                                   /\ pc' = [pc EXCEPT ![self] = Head(stack[self]).pc]
                                                   /\ observed' = [observed EXCEPT ![self] = Head(stack[self]).observed]
                                                   /\ id' = [id EXCEPT ![self] = Head(stack[self]).id]
                                                   /\ stack' = [stack EXCEPT ![self] = Tail(stack[self])]
                             /\ UNCHANGED << graph_state, token_generator,
                                             genlock_state, tokens, op,
                                             next_node, locked >>

LockNode(self) == lock_node_start(self) \/ lock_node_contested(self)

get_token(self) == /\ pc[self] = "get_token"
                        /\ tokens' = [tokens EXCEPT ![self] = token_generator]
                        /\ token_generator' = token_generator + 1
                        /\ pc' = [pc EXCEPT ![self] = "lock_first_node"]
                        /\ UNCHANGED << graph_state, genlock_state, ret_vals,
                                        stack, id, observed, op, next_node,
                                        locked >>

lock_first_node(self) == /\ pc[self] = "lock_first_node"
                         /\ /\ id' = [id EXCEPT ![self] = op[self][1]]
                            /\ stack' = [stack EXCEPT ![self] = << [ procedure |->  "LockNode",
                                                                     pc        |->  "lock_first_node_result",
                                                                     observed  |->  observed[self],
                                                                     id        |->  id[self] ] >>
                                                                 \o stack[self]]
                         /\ observed' = [observed EXCEPT ![self] = defaultInitValue]
                         /\ pc' = [pc EXCEPT ![self] = "lock_node_start"]
                         /\ UNCHANGED << graph_state, token_generator,
                                         genlock_state, ret_vals, tokens,
                                         op, next_node, locked >>

lock_first_node_result(self) == /\ pc[self] = "lock_first_node_result"
                                /\ IF ret_vals[self][1] /= ERR_OK
                                      THEN /\ pc' = [pc EXCEPT ![self] = "unlock"]
                                           /\ UNCHANGED locked
                                      ELSE /\ locked' = [locked EXCEPT ![self] = Append(locked[self], op[self][1])]
                                           /\ pc' = [pc EXCEPT ![self] = "operation_check"]
                                /\ UNCHANGED << graph_state,
                                                token_generator,
                                                genlock_state, ret_vals,
                                                tokens, stack, id,
                                                observed, op, next_node >>

operation_check(self) == /\ pc[self] = "operation_check"
                         /\ IF graph_state[op[self][1]] = op[self][2]
                               THEN /\ pc' = [pc EXCEPT ![self] = "unlock"]
                                    /\ UNCHANGED << ret_vals, next_node >>
                               ELSE /\ IF (graph_state[op[self][1]] /= NULL) /\ (op[self][2] /= NULL)
                                          THEN /\ ret_vals' = [ret_vals EXCEPT ![self][1] = << ERR_INVALID_STATE >>]
                                               /\ UNCHANGED next_node
                                          ELSE /\ next_node' = [next_node EXCEPT ![self] = IF op[self][2] /= NULL THEN op[self][2] ELSE graph_state[op[self][1]]]
                                               /\ UNCHANGED ret_vals
                                    /\ pc' = [pc EXCEPT ![self] = "lock_path"]
                         /\ UNCHANGED << graph_state, token_generator,
                                         genlock_state, tokens, stack, id,
                                         observed, op, locked >>

lock_path(self) == /\ pc[self] = "lock_path"
                   /\ IF next_node[self] /= NULL
                         THEN /\ /\ id' = [id EXCEPT ![self] = next_node[self]]
                                 /\ stack' = [stack EXCEPT ![self] = << [ procedure |->  "LockNode",
                                                                          pc        |->  "lock_path_result",
                                                                          observed  |->  observed[self],
                                                                          id        |->  id[self] ] >>
                                                                      \o stack[self]]
                              /\ observed' = [observed EXCEPT ![self] = defaultInitValue]
                              /\ pc' = [pc EXCEPT ![self] = "lock_node_start"]
                         ELSE /\ pc' = [pc EXCEPT ![self] = "mutate_graph"]
                              /\ UNCHANGED << stack, id, observed >>
                   /\ UNCHANGED << graph_state, token_generator,
                                   genlock_state, ret_vals, tokens, op,
                                   next_node, locked >>

lock_path_result(self) == /\ pc[self] = "lock_path_result"
                          /\ IF ret_vals[self][1] /= ERR_OK
                                THEN /\ pc' = [pc EXCEPT ![self] = "unlock"]
                                     /\ UNCHANGED << next_node, locked >>
                                ELSE /\ locked' = [locked EXCEPT ![self] = Append(locked[self], next_node[self])]
                                     /\ next_node' = [next_node EXCEPT ![self] = graph_state[next_node[self]]]
                                     /\ pc' = [pc EXCEPT ![self] = "lock_path"]
                          /\ UNCHANGED << graph_state, token_generator,
                                          genlock_state, ret_vals, tokens,
                                          stack, id, observed, op >>

mutate_graph(self) == /\ pc[self] = "mutate_graph"
                      /\ graph_state' = [graph_state EXCEPT ![op[self][1]] = op[self][2]]
                      /\ pc' = [pc EXCEPT ![self] = "unlock"]
                      /\ UNCHANGED << token_generator, genlock_state,
                                      ret_vals, tokens, stack, id,
                                      observed, op, next_node, locked >>

unlock(self) == /\ pc[self] = "unlock"
                /\ IF Len(locked[self]) > 0
                      THEN /\ Assert(genlock_state[(Head(locked[self]))] = tokens[self],
                                     "Failure of assertion at line 138, column 3 of macro called at line 240, column 5.")
                           /\ genlock_state' = [genlock_state EXCEPT ![(Head(locked[self]))] = UNLOCKED]
                           /\ locked' = [locked EXCEPT ![self] = Tail(locked[self])]
                           /\ pc' = [pc EXCEPT ![self] = "unlock"]
                      ELSE /\ pc' = [pc EXCEPT ![self] = "maybe_spin"]
                           /\ UNCHANGED << genlock_state, locked >>
                /\ UNCHANGED << graph_state, token_generator, ret_vals,
                                tokens, stack, id, observed, op,
                                next_node >>

maybe_spin(self) == /\ pc[self] = "maybe_spin"
                    /\ IF ret_vals[self][1] = ERR_RETRY
                          THEN /\ pc' = [pc EXCEPT ![self] = "lock_first_node"]
                               /\ UNCHANGED << stack, op, next_node, locked >>
                          ELSE /\ Assert((ret_vals[self][1] /= ERR_CYCLE_DETECTED) \/
                                         ~GraphIsAcyclic(MutatedGraph(ret_vals[self][2], op[self])),
                                         "Failure of assertion at line 256, column 5.")
                               /\ pc' = [pc EXCEPT ![self] = Head(stack[self]).pc]
                               /\ next_node' = [next_node EXCEPT ![self] = Head(stack[self]).next_node]
                               /\ locked' = [locked EXCEPT ![self] = Head(stack[self]).locked]
                               /\ op' = [op EXCEPT ![self] = Head(stack[self]).op]
                               /\ stack' = [stack EXCEPT ![self] = Tail(stack[self])]
                    /\ UNCHANGED << graph_state, token_generator,
                                    genlock_state, ret_vals, tokens, id,
                                    observed >>

PerformOperation(self) == get_token(self) \/ lock_first_node(self)
                             \/ lock_first_node_result(self)
                             \/ operation_check(self) \/ lock_path(self)
                             \/ lock_path_result(self)
                             \/ mutate_graph(self) \/ unlock(self)
                             \/ maybe_spin(self)

update_edge(self) == /\ pc[self] = "update_edge"
                     /\ \E operation \in AllValidRemoveEdgeOperations(INITIAL_PI_GRAPH):
                          /\ /\ op' = [op EXCEPT ![self] = operation]
                             /\ stack' = [stack EXCEPT ![self] = << [ procedure |->  "PerformOperation",
                                                                      pc        |->  "done",
                                                                      next_node |->  next_node[self],
                                                                      locked    |->  locked[self],
                                                                      op        |->  op[self] ] >>
                                                                  \o stack[self]]
                          /\ next_node' = [next_node EXCEPT ![self] = defaultInitValue]
                          /\ locked' = [locked EXCEPT ![self] = << >>]
                          /\ pc' = [pc EXCEPT ![self] = "get_token"]
                     /\ UNCHANGED << graph_state, token_generator,
                                     genlock_state, ret_vals, tokens, id,
                                     observed >>

done(self) == /\ pc[self] = "done"
              /\ TRUE
              /\ pc' = [pc EXCEPT ![self] = "Done"]
              /\ UNCHANGED << graph_state, token_generator, genlock_state,
                              ret_vals, tokens, stack, id, observed, op,
                              next_node, locked >>

threads(self) == update_edge(self) \/ done(self)

(* Allow infinite stuttering to prevent deadlock on termination. *)
Terminating == /\ \A self \in ProcSet: pc[self] = "Done"
               /\ UNCHANGED vars

Next == (\E self \in ProcSet: LockNode(self) \/ PerformOperation(self))
           \/ (\E self \in THREADS: threads(self))
           \/ Terminating

Spec == /\ Init /\ [][Next]_vars
        /\ \A self \in THREADS : /\ WF_vars(threads(self))
                                 /\ WF_vars(PerformOperation(self))
                                 /\ WF_vars(LockNode(self))

Termination == <>(\A self \in ProcSet: pc[self] = "Done")

\* END TRANSLATION

GraphStateIsValid == GraphIsValid(graph_state)

====
