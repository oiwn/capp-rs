# application-rs

Common things i use to build Rust CLI tools for web crawlers.

Configurations for some relatively large crawler tends to be pretty large.


First of all it's possible to load large, complex config from yaml file, which
looks like this:

```yaml
app:
  debug: true
  name: some-crawler
  tasks_concurrency: 400
  tasks_per_run: 1000
  user_agents_path: "data/user_agents.txt"

mongodb:
  default: mongodb://localhost:27017/tiktok

zeromq:
  default: tcp://localhost:5556
```

Load user-agents (i.e. read file line-by-line) and randomly select them.