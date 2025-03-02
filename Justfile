# Common cli tasks

tags:
	ctags -R --exclude=*/*.json --exclude=target/* .

lines:
	tokei

connect-redis:
	docker exec -it redis-stack redis-cli --askpass

connect-mongodb:
    docker exec -it mongodb mongosh

