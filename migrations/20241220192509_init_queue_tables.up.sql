-- Create enum for task status
CREATE TYPE task_status AS ENUM (
    'Queued',
    'InProgress',
    'Completed',
    'Failed',
    'DeadLetter'
);

-- Create main tasks table
CREATE TABLE tasks (
    id UUID PRIMARY KEY,
    payload JSONB NOT NULL,
    status task_status NOT NULL DEFAULT 'Queued',
    queued_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TIMESTAMP WITH TIME ZONE,
    finished_at TIMESTAMP WITH TIME ZONE,
    retries INTEGER NOT NULL DEFAULT 0,
    error_msg TEXT
);

-- Create dead letter queue table with same structure
CREATE TABLE dlq (
    id UUID PRIMARY KEY,
    payload JSONB NOT NULL,
    status task_status NOT NULL DEFAULT 'DeadLetter',
    queued_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TIMESTAMP WITH TIME ZONE,
    finished_at TIMESTAMP WITH TIME ZONE,
    retries INTEGER NOT NULL DEFAULT 0,
    error_msg TEXT
);

-- Create index for finding available tasks efficiently
CREATE INDEX idx_tasks_status ON tasks(status) WHERE status = 'Queued';
