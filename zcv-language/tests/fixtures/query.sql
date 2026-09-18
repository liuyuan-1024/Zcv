-- 活跃用户统计
WITH active_users AS (
    SELECT id, name, department_id
    FROM users
    WHERE active = TRUE
)
SELECT department_id, COUNT(*) AS total, MAX(name) AS last_name
FROM active_users
GROUP BY department_id
HAVING COUNT(*) > 3
ORDER BY total DESC;
