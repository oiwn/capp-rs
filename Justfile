# Common cli tasks

tags:
	ctags -R --exclude=*/*.json --exclude=target/* .

lines:
	pygount --format=summary --folders-to-skip=target,data,__pycache__,.git --names-to-skip=tags,*.html

connect-redis:
	docker exec -it redis-stack redis-cli --askpass
