# Software system description method

Describe a system through seven views. Choose diagrams, tables, and detail that help each audience understand it. A small desktop app does not need every example listed here.

| View | Readers | Main question |
| --- | --- | --- |
| 1. Business | Sponsors, product owners, users | Why does the system exist, and what value does it provide? |
| 2. System / context | All stakeholders | Who uses it, what does it connect to, and where does its scope end? |
| 3. Architecture / logical structure | Architects and technical leads | What are the main parts, and what does each do? |
| 4. Process / behavior | Developers and integration/performance specialists | How do parts work together while the system runs? |
| 5. Data / information | Developers, data architects, compliance reviewers | What data is used, stored, shared, and protected? |
| 6. Implementation / development | Developers and technical leads | How is the source built, organized, and maintained? |
| 7. Deployment / technology | Operations, infrastructure, security | Where does it run, and what does it need? |

## 1. Business

Explain goals, capabilities, stakeholders, and user tasks. Show how the system supports the organization's goals. Useful examples include a capability map, high-level process map, role/responsibility table (RACI), user journeys, success measures (KPIs), and cost-benefit or return-on-investment estimates.

## 2. System / context

Show the system from the outside: users, other systems, boundaries, and external connections. Use a context diagram, actor list, data-flow overview, scope statement, or interface list covering APIs, events, and files. A C4 Level 1 diagram is one option.

## 3. Architecture / logical structure

Explain the main components, layers, modules, or services and their responsibilities. Focus on the design before technology details where possible. Use component/container diagrams (C4 Levels 2–3), layer diagrams, responsibility tables, internal interface contracts, design decisions (ADRs), and security, performance, or scaling scenarios.

## 4. Process / behavior

Explain runtime interactions, workflows, threads, events, and timing. Use sequence diagrams for individual tasks, activity/BPMN diagrams for processes, state machines for important objects, and thread/task diagrams. Include performance and scaling limits where relevant.

## 5. Data / information

Explain data structure, ownership, flow, storage, and quality. Use conceptual, logical, or physical data models (ERDs), data-flow diagrams, field dictionaries, ownership/origin notes, and storage/retention rules. Describe master and reference data where the system uses them.

## 6. Implementation / development

Explain source folders, repositories, modules, build units, dependencies, and relevant technologies. Use a package/module diagram, source tree, build graph, coding guidelines, or a table linking design components to code and owners. Describe separate teams only when they exist.

## 7. Deployment / technology

Explain machines, networks, services, and operating needs. Use deployment diagrams, infrastructure/security-zone diagrams, and development/test/production layouts. Cover scaling, availability, failover, recovery, logs, monitoring, and runtime technology choices where applicable. Show containers, VMs, or Kubernetes pods only when used.

## Keep the views useful

Choose detail to suit the system and readers. Keep component names consistent. Split large diagrams so each explains one flow or responsibility. The C4 model—Context → Containers → Components → Code—can help organize the context, architecture, and implementation views.
